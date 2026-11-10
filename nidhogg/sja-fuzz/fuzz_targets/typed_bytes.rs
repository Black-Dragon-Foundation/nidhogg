//! Arbitrary bytes as a `view-model` line's model: whatever the typed read reads, the `Value` read reads the same.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sja::queue::{read_line_typed, read_line_value};

fuzz_target!(|data: &[u8]| {
    let Ok(model) = std::str::from_utf8(data) else {
        return;
    };
    let line = format!(
        "{{\"event\":\"view-model\",\"data\":{{\"base\":0,\"hub\":1,\"model\":{model},\"version\":1,\"view\":\"v\"}}}}"
    );
    for text in [line.as_str(), model] {
        if let Some((e, ops)) = read_line_typed(text) {
            assert_eq!(read_line_value(text), Some((e, Ok(ops))), "{text}");
        }
    }
});
