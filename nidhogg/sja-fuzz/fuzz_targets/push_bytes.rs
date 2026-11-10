//! Arbitrary bytes as a `view-model` line's model, pushed and drained through `sja_call`: never a panic past the guard.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sja::abi::{
    Ans, SJA_CLOSE, SJA_DRAIN, SJA_OPEN, SJA_PUSH, SJA_STATS, call_for_test, int, nil, text,
};

fuzz_target!(|data: &[u8]| {
    let Ok(model) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(Ans::Int(h)) = call_for_test(SJA_OPEN, &[text("fuzz-push"), nil()]) else {
        return;
    };
    for (base, version, reset) in [(0, 1, ",\"reset\":true"), (1, 2, "")] {
        let line = format!(
            "{{\"event\":\"view-model\",\"data\":{{\"base\":{base},\"hub\":1,\"model\":{model}{reset},\"version\":{version},\"view\":\"fuzz-push\"}}}}"
        );
        let _ = call_for_test(SJA_PUSH, &[text(&line)]);
    }
    let _ = call_for_test(SJA_DRAIN, &[text("fuzz-push"), nil()]);
    if let Ok(Ans::List(xs)) = call_for_test(SJA_STATS, &[]) {
        assert_eq!(xs.get(3), Some(&Ans::Int(0)), "a panic reached the guard");
    }
    let _ = call_for_test(SJA_CLOSE, &[int(h)]);
});
