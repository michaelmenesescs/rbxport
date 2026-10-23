//! Which volumes are offered as export destinations.
//!
//! Its own test binary: it sets [`rbl_devices::FAKE_VOLUMES`], a process-wide
//! variable the unit tests also set.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn a_stick_with_a_publication_journal_is_listed() {
    // A stick left mid-publish: the export it holds is not read until it is
    // recovered, but it is still somewhere to export to, and the export is
    // what recovers it.
    let dir = tempfile::tempdir().unwrap();
    let stick = dir.path().join("LEXAR");
    let journal = stick.join(".rbxport-publication");
    std::fs::create_dir_all(&journal).unwrap();
    std::fs::write(journal.join("publication.json"), b"[]").unwrap();
    std::fs::write(journal.join(".verification-required"), b"1").unwrap();
    assert!(rbl_export::recovery_pending(&stick));

    std::env::set_var(rbl_devices::FAKE_VOLUMES, &stick);
    let devices = rbl_devices::list();
    std::env::remove_var(rbl_devices::FAKE_VOLUMES);

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].mount_point, stick);
    assert_eq!(rbl_devices::inspect(&stick), None);
}
