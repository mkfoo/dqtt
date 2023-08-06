# DQTT
Absolutely trivial publish/subscribe messaging protocol over Unix domain sockets.

There are only two message types:

    SUBSCRIBE => "\x01{topic_bytes}\x04"
        where {topic_bytes} is less than MAX_TOPIC_BYTES in length and does not contain '\x03' or '\x04'
  
    PUBLISH => "\x02{topic_bytes}\x03{payload_bytes}\x04"
        where {topic_bytes} is less than MAX_TOPIC_BYTES in length and does not contain '\x03' or '\x04'
        and {payload_bytes} is less than MAX_PAYLOAD_BYTES in length and does not contain '\x04'
