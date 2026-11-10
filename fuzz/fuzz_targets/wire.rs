#![no_main]
use core_contracts::{MAX_FRAME, Request, read_frame, strict_json};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FRAME {
        return;
    }
    let _ = strict_json(data);
    let _ = core_engines::inspect_manifest(data);
    let mut connection = core_runtime::Connection::default();
    let mut input = Cursor::new(data);
    // Explore short stateful conversations without turning a single mutation
    // into a million-frame workload. Production framing/queues have own bounds.
    for _ in 0..32 {
        match read_frame(&mut input) {
            Ok(Some(frame)) => {
                // Keep future mutating/session operations out of this target,
                // even after real engines and installers become available.
                let pure = Request::decode(&frame).map_or(true, |request| {
                    matches!(
                        request.operation.as_str(),
                        "hello" | "capabilities.get" | "engines.list" | "launch.plan"
                    )
                });
                if !pure {
                    continue;
                }
                let response = connection.handle(&frame);
                // Wire responses must remain serializable for every input.
                serde_json::to_vec(&response).expect("Response serialization");
            }
            Ok(None) | Err(_) => break,
        }
    }
});
