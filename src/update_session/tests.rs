use super::*;
use crate::update::Version;

fn update() -> Update {
    Update {
        version: Version::parse("9.0.0").unwrap_or_else(|| panic!("fixture version")),
        url: "fixture".into(),
    }
}

fn background_download(now: Instant) -> UpdateSession {
    let mut session = UpdateSession::new(now);
    assert_eq!(session.tick(true, false, now, || false), Effect::None);
    assert_eq!(
        session.tick(true, false, now + FIRST_CHECK, || false),
        Effect::Check { manual: false }
    );
    assert_eq!(
        session.checked(Ok(Checked::Available(update())), now + FIRST_CHECK),
        Effect::Stage(update())
    );
    session
}

#[test]
fn opening_completed_status_does_not_start_another_check() {
    let now = Instant::now();
    for result in [
        Ok(Checked::UpToDate),
        Ok(Checked::Blocked(
            update().version,
            "folder restriction".into(),
        )),
        Err("request timed out".into()),
    ] {
        let mut session = UpdateSession::new(now);
        assert_eq!(session.open(true, now), Effect::Check { manual: true });
        assert_eq!(session.checked(result, now), Effect::None);
        let retained = session.view("1.3.0");
        // Hiding has no policy event. Opening it again must leave the completion intact.
        for _ in 0..3 {
            assert_eq!(session.open(true, now), Effect::None);
            assert_eq!(session.view("1.3.0"), retained);
        }
        assert_eq!(
            session.command(Command::Check, now),
            Effect::Check { manual: true }
        );
    }
}

#[test]
fn observing_a_check_requires_an_offer_and_explicit_install_approval() {
    let now = Instant::now();
    let mut session = UpdateSession::new(now);
    session.tick(true, false, now, || false);
    assert_eq!(
        session.tick(true, false, now + FIRST_CHECK, || false),
        Effect::Check { manual: false }
    );
    assert_eq!(session.open(true, now), Effect::None);
    assert_eq!(
        session.checked(Ok(Checked::Available(update())), now),
        Effect::None
    );
    assert_eq!(
        session.command(Command::Install, now),
        Effect::Stage(update())
    );
    assert!(session.restart.approved());
    session.staged(Ok(()), now);
    assert_eq!(
        session.tick(true, true, now, || panic!(
            "approved restart needs no idle check"
        )),
        Effect::PrepareRestart
    );
}

#[test]
fn observing_background_download_never_approves_immediate_restart() {
    let now = Instant::now();
    let mut session = background_download(now);
    assert_eq!(session.open(true, now), Effect::None);
    assert!(!session.restart.approved());
    session.staged(Ok(()), now);
    assert_eq!(session.open(true, now), Effect::None);
    assert_eq!(session.tick(true, true, now, || false), Effect::None);
    assert_eq!(session.tick(false, true, now, || true), Effect::None);
    session.command(Command::Restart, now);
    assert_eq!(
        session.tick(false, false, now, || panic!("switcher gate")),
        Effect::None
    );
    assert_eq!(
        session.tick(false, true, now, || panic!("explicit approval")),
        Effect::PrepareRestart
    );
}

#[test]
fn helper_readiness_does_not_override_user_return_or_disabled_automatic_updates() {
    let now = Instant::now();
    let mut session = background_download(now);
    session.staged(Ok(()), now);
    assert_eq!(
        session.tick(true, true, now, || true),
        Effect::PrepareRestart
    );
    session.prepared(Ok(()), now);
    assert_eq!(session.tick(true, true, now, || false), Effect::None);
    assert_eq!(session.tick(false, true, now, || true), Effect::None);
    session.command(Command::Restart, now);
    assert_eq!(
        session.tick(false, true, now, || panic!("explicit approval")),
        Effect::ApproveRestart
    );
}

#[test]
fn staged_failure_keeps_retry_offer_and_day_cooldown() {
    let now = Instant::now();
    let mut session = background_download(now);
    session.staged(Err(Failure::new(Stage::Extract, "disk full")), now);
    assert_eq!(
        session.view("1.3.0").primary,
        Some(("Retry and restart", Command::Install))
    );
    assert_eq!(
        session.tick(
            true,
            true,
            now + INTERVAL.saturating_sub(Duration::from_secs(1)),
            || { false }
        ),
        Effect::None
    );
    assert_eq!(
        session.tick(true, true, now + INTERVAL, || false),
        Effect::Check { manual: false }
    );
}

#[test]
fn disconnected_download_keeps_a_retryable_offer() {
    let now = Instant::now();
    let mut session = background_download(now);
    session.worker_failed(now);
    assert_eq!(
        session.command(Command::Install, now),
        Effect::Stage(update())
    );
    assert!(session.restart.approved());
}

#[test]
fn disabling_automatic_updates_mid_check_preserves_manual_offer() {
    let now = Instant::now();
    let mut session = UpdateSession::new(now);
    session.tick(true, false, now, || false);
    session.tick(true, false, now + FIRST_CHECK, || false);
    session.tick(false, false, now, || false);
    assert_eq!(
        session.checked(Ok(Checked::Available(update())), now),
        Effect::None
    );
    assert_eq!(session.open(true, now), Effect::None);
    assert_eq!(
        session.command(Command::Install, now),
        Effect::Stage(update())
    );
}

#[test]
fn local_build_status_never_starts_a_check_on_open() {
    let now = Instant::now();
    let mut session = UpdateSession::new(now);
    assert_eq!(session.open(false, now), Effect::None);
    assert_eq!(session.status, Status::LocalBuild);
    assert!(session.has_status());
}
