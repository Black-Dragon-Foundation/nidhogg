//! Arbitrary bytes as a model patch through `sja_call`, then a few verbs: never a panic past the guard (a panic inside
//! one would show as a crash here only if it escaped), never over the caps.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sja::abi::{SJA_APPLY, SJA_CLOSE, SJA_FOLLOW, SJA_GET, SJA_OPEN, SJA_WALK, call_for_test, int, text};

fuzz_target!(|data: &[u8]| {
    let Ok(patch) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(sja::abi::Ans::Int(h)) = call_for_test(SJA_OPEN, &[text("fuzz"), sja::abi::nil()]) else {
        return;
    };
    let _ = call_for_test(SJA_APPLY, &[text("fuzz"), text("h"), int(1), text(patch)]);
    for link in ["up", "down", "left", "right"] {
        let _ = call_for_test(SJA_FOLLOW, &[int(h), text(link)]);
    }
    let _ = call_for_test(SJA_WALK, &[int(h), text("all"), int(1)]);
    let _ = call_for_test(SJA_GET, &[int(h), text("classes")]);
    let stats = call_for_test(sja::abi::SJA_STATS, &[]);
    if let Ok(sja::abi::Ans::List(xs)) = stats {
        assert_eq!(xs.get(3), Some(&sja::abi::Ans::Int(0)), "a panic reached the guard");
    }
    let _ = call_for_test(SJA_CLOSE, &[int(h)]);
});
