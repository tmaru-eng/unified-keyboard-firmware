//! A key the user is no longer holding must never stay down on the host.
//!
//! A bridge cannot tell a held key from a dead link by looking at its input
//! alone: a HID keyboard reports changes, so holding Shift for a minute and
//! losing the link while Shift is down produce exactly the same silence. The
//! difference is whether anything else still proves the link is alive, which is
//! why this watchdog is fed liveness separately from reports.

use ukf_core::{BootKeyboardReport, StuckKeyWatchdog};

const LIMIT_MS: u64 = 5_000;

fn shift_down() -> BootKeyboardReport {
    BootKeyboardReport {
        modifiers: 0b0000_0010,
        keys: [0; 6],
    }
}

fn a_down() -> BootKeyboardReport {
    BootKeyboardReport {
        modifiers: 0,
        keys: [0x04, 0, 0, 0, 0, 0],
    }
}

#[test]
fn nothing_held_never_expires() {
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);

    watchdog.observe_output(BootKeyboardReport::EMPTY, 0);

    assert_eq!(watchdog.deadline(), None);
    assert!(!watchdog.expired(u64::MAX));
}

#[test]
fn a_held_key_with_no_further_evidence_expires_after_the_limit() {
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);

    watchdog.observe_output(shift_down(), 1_000);

    assert_eq!(watchdog.deadline(), Some(1_000 + LIMIT_MS));
    assert!(!watchdog.expired(1_000 + LIMIT_MS - 1));
    assert!(watchdog.expired(1_000 + LIMIT_MS));
}

#[test]
fn liveness_keeps_a_deliberately_held_key_down() {
    // Holding a modifier produces no further reports, because a HID keyboard
    // sends changes. Releasing it after a few seconds would be the watchdog
    // typing for the user, so proof that the keyboard is still answering has to
    // hold the deadline open.
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);
    watchdog.observe_output(shift_down(), 0);

    for second in 1..=60 {
        let now = second * 1_000;
        watchdog.observe_liveness(now);
        assert!(!watchdog.expired(now), "released a key still being held");
    }

    assert!(watchdog.expired(60_000 + LIMIT_MS));
}

#[test]
fn liveness_without_a_held_key_arms_nothing() {
    // Evidence that the link is alive is not a reason to start a countdown; a
    // deadline only exists while the host is holding something.
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);
    watchdog.observe_output(BootKeyboardReport::EMPTY, 0);

    watchdog.observe_liveness(1_000);

    assert_eq!(watchdog.deadline(), None);
}

#[test]
fn a_new_report_is_itself_evidence() {
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);
    watchdog.observe_output(shift_down(), 0);

    watchdog.observe_output(a_down(), 4_000);

    assert_eq!(watchdog.deadline(), Some(4_000 + LIMIT_MS));
}

#[test]
fn releasing_everything_disarms_the_watchdog() {
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);
    watchdog.observe_output(a_down(), 0);

    watchdog.observe_output(BootKeyboardReport::EMPTY, 100);

    assert_eq!(watchdog.deadline(), None);
    assert!(!watchdog.expired(u64::MAX));
}

#[test]
fn an_expired_watchdog_stays_expired_until_the_release_is_observed() {
    // The caller reports what it actually emitted. Clearing the deadline on the
    // decision rather than on the emitted report would disarm the watchdog even
    // when the release could not be written, which is the one case where it
    // still has work to do.
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);
    watchdog.observe_output(a_down(), 0);

    assert!(watchdog.expired(LIMIT_MS));
    assert!(watchdog.expired(LIMIT_MS + 10_000));

    watchdog.observe_output(BootKeyboardReport::EMPTY, LIMIT_MS + 10_000);

    assert!(!watchdog.expired(u64::MAX));
}

#[test]
fn a_modifier_alone_arms_the_watchdog() {
    // The failure that motivated this left a meta key down, not a letter. A
    // report with no key slots filled is still a report the host is holding.
    let mut watchdog = StuckKeyWatchdog::new(LIMIT_MS);

    watchdog.observe_output(shift_down(), 0);

    assert_eq!(watchdog.deadline(), Some(LIMIT_MS));
}
