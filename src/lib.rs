use log::{debug, error, warn};
use nix::sys::epoll::{
    epoll_create1, epoll_ctl, epoll_wait, EpollCreateFlags, EpollEvent, EpollFlags, EpollOp,
};
use std::collections::{hash_map::Entry, HashMap};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::fd::{FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

type Topic = u64;

const MAX_TOPICS: usize = std::mem::size_of::<Topic>() * 8;
const MAX_TOPIC_BYTES: u64 = 128;
const MAX_PAYLOAD_BYTES: u64 = 1024;
const MAX_EVENTS: usize = 128;
const READ_TIMEOUT: u64 = 100;
const MAX_FD: RawFd = 524288;

const NUL: u8 = 0;
const SUB: u8 = 1;
const PUB: u8 = 2;
const SEP: u8 = 3;
const END: u8 = 4;

pub fn run<P: AsRef<Path>>(sockpath: P) -> Result<()> {
    self::logger::init();
    let _ = std::fs::remove_file(&sockpath);
    let listener = UnixListener::bind(&sockpath)?;
    let epfd = epoll_create1(EpollCreateFlags::EPOLL_CLOEXEC)?;
    let server = thread::spawn(move || Server::run(epfd));
    assert!(!server.is_finished());

    for res in listener.incoming() {
        if server.is_finished() {
            error!("server thread died unexpectedly");
            break;
        }
        let stream = res?;
        let fd = stream.into_raw_fd();
        assert!(fd > 2 && fd < MAX_FD);
        let mut event = EpollEvent::new(EpollFlags::EPOLLIN | EpollFlags::EPOLLRDHUP, fd as u64);
        epoll_ctl(epfd, EpollOp::EpollCtlAdd, fd, Some(&mut event))?;
    }

    let _ = server.join();
    Ok(nix::unistd::close(epfd)?)
}

struct Server {
    epfd: RawFd,
    topics: HashMap<Vec<u8>, Topic>,
    conns: HashMap<RawFd, (Topic, BufReader<UnixStream>)>,
    topic: Vec<u8>,
    payload: Vec<u8>,
}

impl Server {
    fn new(epfd: RawFd) -> Self {
        Self {
            epfd,
            topics: HashMap::new(),
            conns: HashMap::new(),
            topic: Vec::with_capacity(MAX_TOPIC_BYTES as usize),
            payload: Vec::with_capacity(MAX_PAYLOAD_BYTES as usize),
        }
    }

    fn run(epfd: RawFd) {
        let mut server = Self::new(epfd);
        let mut events = vec![EpollEvent::empty(); MAX_EVENTS];

        loop {
            if let Err(e) = server.main(&mut events) {
                error!("{}", e);
                break;
            }
        }
    }

    fn main(&mut self, events: &mut [EpollEvent]) -> Result<()> {
        let n = epoll_wait(self.epfd, events, -1)?;

        for e in &events[0..n] {
            let fd = e.data() as RawFd;
            assert!(fd > 2 && fd < MAX_FD);

            self.connect(fd)?;

            if e.events().contains(EpollFlags::EPOLLIN) {
                match self.read_message(fd) {
                    Ok(SUB) => self.subscribe(fd)?,
                    Ok(PUB) => self.publish(fd)?,
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                        debug!("eof (fd: {})", fd);
                        self.disconnect(fd)?;
                        continue;
                    }
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => {
                        warn!("connection reset (fd: {})", fd);
                        self.disconnect(fd)?;
                        continue;
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        debug!(
                            "timed out (fd: {}, topic: {}, payload: {})",
                            fd,
                            String::from_utf8_lossy(&self.topic),
                            String::from_utf8_lossy(&self.payload)
                        );
                    }
                    Err(e) => return Err(Box::new(e)),
                }
            }

            if e.events()
                .intersects(EpollFlags::EPOLLERR | EpollFlags::EPOLLHUP | EpollFlags::EPOLLRDHUP)
            {
                self.disconnect(fd)?;
            }
        }

        Ok(())
    }

    fn connect(&mut self, fd: RawFd) -> Result<()> {
        if let Entry::Vacant(e) = self.conns.entry(fd) {
            let stream = unsafe { UnixStream::from_raw_fd(fd) };
            stream.set_read_timeout(Some(Duration::from_millis(READ_TIMEOUT)))?;
            e.insert((0, BufReader::new(stream)));
            debug!("connect (fd: {})", fd);
        }

        Ok(())
    }

    fn disconnect(&mut self, fd: RawFd) -> Result<()> {
        epoll_ctl(self.epfd, EpollOp::EpollCtlDel, fd, None)?;
        self.conns.remove(&fd);
        debug!("disconnect (fd: {})", fd);
        Ok(())
    }

    fn read_message(&mut self, fd: RawFd) -> std::io::Result<u8> {
        let (_, ref mut stream) = self.conns.get_mut(&fd).unwrap();
        let mut start = [0_u8; 1];
        self.topic.clear();
        self.payload.clear();

        loop {
            stream.read_exact(&mut start)?;

            match start[0] {
                SUB => {
                    stream
                        .take(MAX_TOPIC_BYTES)
                        .read_until(END, &mut self.topic)?;

                    if self.topic.ends_with(&[END]) {
                        self.topic.pop().unwrap();
                        break Ok(SUB);
                    }

                    warn!("truncated message (SUB) (fd: {})", fd);
                    break Ok(NUL);
                }
                PUB => {
                    stream
                        .take(MAX_TOPIC_BYTES)
                        .read_until(SEP, &mut self.topic)?;
                    stream
                        .take(MAX_PAYLOAD_BYTES)
                        .read_until(END, &mut self.payload)?;

                    if self.topic.ends_with(&[SEP]) && self.payload.ends_with(&[END]) {
                        self.topic.pop().unwrap();
                        break Ok(PUB);
                    }

                    warn!("truncated message (PUB) (fd: {})", fd);
                    break Ok(NUL);
                }
                NUL => break Ok(NUL),
                _ => {}
            }
        }
    }

    fn get_topic(&mut self) -> u64 {
        if let Some(flag) = self.topics.get(&self.topic) {
            *flag
        } else if self.topics.len() < MAX_TOPICS {
            let flag = 1_u64 << self.topics.len();
            self.topics.insert(self.topic.clone(), flag);
            flag
        } else {
            panic!("too many topics");
        }
    }

    fn subscribe(&mut self, fd: RawFd) -> Result<()> {
        let flag = self.get_topic();
        let (ref mut subs, _) = self.conns.get_mut(&fd).unwrap();
        *subs |= flag;
        debug!(
            "subscribe (fd: {}, topic: {:?})",
            fd,
            String::from_utf8_lossy(&self.topic)
        );
        Ok(())
    }

    fn publish(&mut self, fd: RawFd) -> Result<()> {
        let flag = self.get_topic();

        for (_, (_, stream)) in self
            .conns
            .iter_mut()
            .filter(|(k, (s, _))| **k != fd && s & flag == flag)
        {
            let writer = stream.get_mut();
            writer.write_all(&self.payload)?;
        }

        debug!(
            "publish (fd: {}, topic: {}, payload: {})",
            fd,
            String::from_utf8_lossy(&self.topic),
            String::from_utf8_lossy(&self.payload)
        );
        Ok(())
    }
}

pub struct Client {
    stream: BufReader<UnixStream>,
    buf: Vec<u8>,
    mode: i32,
}

impl Client {
    pub fn connect(path: &str) -> Self {
        Self {
            stream: BufReader::new(UnixStream::connect(path).unwrap()),
            buf: vec![],
            mode: 0,
        }
    }

    pub fn subscribe(&mut self, topic: &[u8]) -> std::io::Result<()> {
        let writer = self.stream.get_mut();
        writer.write_all(&[&[SUB], topic, &[END]].concat())
    }

    pub fn publish(&mut self, topic: &[u8], payload: &[u8]) -> std::io::Result<()> {
        let writer = self.stream.get_mut();
        writer.write_all(&[&[PUB], topic, &[SEP], payload, &[END]].concat())
    }

    fn change_mode(&mut self, timeout: i32) -> std::io::Result<()> {
        let mode = match timeout {
            t if t < 0 => -1,
            0 => 0,
            _ => 1,
        };

        if self.mode != mode {
            let s = self.stream.get_mut();

            match mode {
                -1 => {
                    s.set_nonblocking(true)?;
                }
                0 => {
                    s.set_nonblocking(false)?;
                    s.set_read_timeout(None)?;
                }
                _ => {
                    s.set_nonblocking(false)?;
                    let dur = Duration::from_millis(timeout as u64);
                    s.set_read_timeout(Some(dur))?;
                }
            }

            self.mode = mode;
        }

        Ok(())
    }

    pub fn wait(&mut self, timeout: i32) -> std::io::Result<Option<&[u8]>> {
        self.change_mode(timeout)?;
        self.buf.clear();

        match self.stream.read_until(END, &mut self.buf) {
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
            Ok(0) => return Err(std::io::Error::from(ErrorKind::UnexpectedEof)),
            _ => {}
        }

        if self.buf.ends_with(&[END]) {
            self.buf.pop().unwrap();
            Ok(Some(&self.buf))
        } else {
            Ok(None)
        }
    }

    pub fn expect(&mut self, expected: &[u8], timeout: i32) {
        let msg = self.wait(timeout).unwrap();
        let exp = Some(expected);
        assert_eq!(exp, msg);
    }
}

mod logger {
    use log::{Metadata, Record, STATIC_MAX_LEVEL};

    struct Logger;

    static LOGGER: Logger = Logger;

    pub fn init() {
        log::set_logger(&LOGGER)
            .map(|()| log::set_max_level(STATIC_MAX_LEVEL))
            .unwrap();
    }

    impl log::Log for Logger {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= STATIC_MAX_LEVEL
        }

        fn log(&self, record: &Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }

        fn flush(&self) {}
    }
}
