//! Its own test binary: setting `TZ` changes the whole process.

use pr_review_tui::waterfall::local_time;

unsafe extern "C" {
    fn tzset();
}

#[test]
fn daemon_timestamps_read_in_the_machines_time_zone() {
    // SAFETY: this binary's only test, so nothing else reads the environment.
    unsafe {
        std::env::set_var("TZ", "Europe/Madrid");
        tzset();
    }
    let local = |at| local_time(at).unwrap();
    assert_eq!(local("2026-11-02T11:01:07.444Z"), "2026-11-02 12:01:07");
    assert_eq!(local("2026-07-01T11:01:07Z"), "2026-07-01 13:01:07", "summer time");
    assert_eq!(local("2026-11-02T23:30:00Z"), "2026-11-03 00:30:00", "the next local day");
    assert_eq!(local_time("not a time"), None);
}
