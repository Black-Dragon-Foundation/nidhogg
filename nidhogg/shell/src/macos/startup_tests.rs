//! Startup over a process's life as the glue runs it: every entry point runs inside its guard's own pool
//! (`crate::with_pool`), so a pool made during one call is drained when that call returns.

use super::*;

/// Startup, a later call that autoreleases (loadup's calls do), then startup again (a re-exec'd daemon child), each
/// in its guard's pool, with nothing for the end of the process to release.  A pool startup kept past its call was
/// drained with the guard's pool, and releasing it at `kill-emacs` was objc's fatal "prematurely-freed autorelease
/// pool", which killed the bootstrap dump.
#[test]
fn startup_keeps_no_pool_past_its_call() {
    crate::with_pool(init);
    crate::with_pool(|| {
        let _ = NSLocale::currentLocale().localeIdentifier();
    });
    crate::with_pool(init);
}
