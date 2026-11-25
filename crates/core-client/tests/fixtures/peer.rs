use core_contracts::{MAX_FRAME, Request, Response, read_frame};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, Read, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn request(input: &mut impl BufRead, operation: &str) -> Request {
    let frame = read_frame(input).unwrap().expect("request frame");
    let request = Request::decode(&frame).unwrap();
    assert_eq!(request.operation, operation);
    request
}

fn reply(output: &mut impl Write, id: String, result: Value) {
    serde_json::to_writer(&mut *output, &Response::new(Some(id), Ok(result))).unwrap();
    output.write_all(b"\n").unwrap();
    output.flush().unwrap();
}

fn main() {
    let mut args = env::args_os().skip(1);
    assert_eq!(args.next().unwrap(), "serve");
    let scenario = PathBuf::from(args.next().unwrap());
    let work = PathBuf::from(args.next().unwrap());
    assert!(args.next().is_none());
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let hello = request(&mut input, "hello");
    match scenario.file_name().unwrap().to_str().unwrap() {
        "wrong-response-id" => {
            reply(&mut output, "other-id".into(), json!({}));
            assert_eq!(input.read(&mut [0]).unwrap(), 0);
            fs::write(work.join("eof"), b"").unwrap();
        }
        "stdin-backpressure" => {
            reply(
                &mut output,
                hello.id,
                json!({"product":"Numera Core Browser","core_version":"fixture","protocol":1,"schema_revision":1}),
            );
            let capability = request(&mut input, "capabilities.get");
            reply(
                &mut output,
                capability.id,
                json!({"namespace":"numera.core","core_version":"fixture","protocol":1,"schema_revision":1,
                    "host":{"os":env::consts::OS,"arch":env::consts::ARCH},
                    "scope":{"engine_build":null,"device_mode":null,"automation_transport":null},
                    "features":{"protocol.ndjson":{"status":"supported"}}}),
            );
            let drain = work.join("drain");
            let failsafe = Instant::now() + Duration::from_secs(3);
            while !drain.is_file() && Instant::now() < failsafe {
                thread::sleep(Duration::from_millis(10));
            }
            let count = io::copy(&mut input, &mut io::sink()).unwrap();
            fs::write(work.join("bytes"), count.to_string()).unwrap();
        }
        "unterminated-frame" => {
            output.write_all(&vec![b'x'; MAX_FRAME]).unwrap();
            output.flush().unwrap();
            assert_eq!(input.read(&mut [0]).unwrap(), 0);
        }
        "duplicate-key" => {
            writeln!(
                output,
                r#"{{"namespace":"numera.core","protocol":1,"id":"{}","status":"ok","result":{{"a":1,"a":2}}}}"#,
                hello.id
            )
            .unwrap();
            output.flush().unwrap();
            assert_eq!(input.read(&mut [0]).unwrap(), 0);
        }
        _ => panic!("unknown NCB-1 peer fixture"),
    }
}
