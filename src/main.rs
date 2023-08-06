fn main() -> dqtt::Result<()> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();

    if args.len() != 2 {
        panic!(
            "expected 1 argument, found {}",
            args.len().saturating_sub(1)
        );
    }

    dqtt::run(&args[1])
}

#[cfg(test)]
mod tests {
    use dqtt::Client;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn two_clients_one_topic() {
        let server = thread::spawn(|| dqtt::run("./sock").unwrap());
        thread::sleep(Duration::from_millis(200));
        let c1 = thread::spawn(|| {
            let mut client = Client::connect("./sock");
            client.subscribe(b"topic1");
            client.expect(b"message1", 0);
            thread::sleep(Duration::from_millis(200));
        });
        thread::sleep(Duration::from_millis(200));
        let c2 = thread::spawn(|| {
            let mut client = Client::connect("./sock");
            client.publish(b"topic1", b"message1");
            thread::sleep(Duration::from_millis(200));
        });
        c2.join().unwrap();
        c1.join().unwrap();
        server.join().unwrap();
    }
}
