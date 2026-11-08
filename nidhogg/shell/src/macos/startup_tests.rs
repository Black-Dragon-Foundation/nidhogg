//! Startup over a process's life as the glue runs it: every entry point runs inside its guard's own pool
//! (`crate::with_pool`), so a pool made during one call is drained when that call returns.

use super::*;

/// Startup, a later call that autoreleases (loadup's calls do), then the end of the process, each in its guard's
/// pool.  A pool startup kept past its call is drained with the guard's pool, and releasing it at the end is objc's
/// fatal "prematurely-freed autorelease pool", which killed the bootstrap dump at `kill-emacs`.
#[test]
fn startup_keeps_no_pool_past_its_call() {
    crate::with_pool(init);
    crate::with_pool(|| {
        let _ = NSLocale::currentLocale().localeIdentifier();
    });
    crate::with_pool(shutdown);
}
