//! glab preflight: 미설치 / 미인증 / 구버전 / ready 구분(gh preflight 미러).

mod glab_fixture;

use glab_fixture::{activate_no_glab, env_lock, FakeGlab};
use suaegi_forge::{glab_preflight, GlabPreflight, GlabRunner};

#[tokio::test]
async fn preflight_ready_when_installed_and_authed() {
    let _g = env_lock();
    let fake = FakeGlab::new()
        .rule("--version", "glab version 1.36.0 (2024-05-01)\n", "", 0)
        .rule("auth status", "", "gitlab.com\n  Logged in as octocat\n", 0);
    let _p = fake.activate();
    assert_eq!(glab_preflight(&GlabRunner::new()).await, GlabPreflight::Ready);
}

#[tokio::test]
async fn preflight_not_installed_when_glab_absent() {
    let _g = env_lock();
    let _p = activate_no_glab();
    assert_eq!(
        glab_preflight(&GlabRunner::new()).await,
        GlabPreflight::NotInstalled
    );
}

#[tokio::test]
async fn preflight_not_authenticated_when_auth_status_fails() {
    let _g = env_lock();
    let fake = FakeGlab::new()
        .rule("--version", "glab version 1.36.0 (2024-05-01)\n", "", 0)
        .rule(
            "auth status",
            "",
            "No token provided. Run `glab auth login` to authenticate.\n",
            1,
        );
    let _p = fake.activate();
    assert_eq!(
        glab_preflight(&GlabRunner::new()).await,
        GlabPreflight::NotAuthenticated
    );
}

#[tokio::test]
async fn preflight_outdated_version_is_flagged() {
    let _g = env_lock();
    let fake = FakeGlab::new()
        .rule("--version", "glab version 1.10.0 (2022-01-01)\n", "", 0)
        .rule("auth status", "", "Logged in\n", 0);
    let _p = fake.activate();
    match glab_preflight(&GlabRunner::new()).await {
        GlabPreflight::OutdatedVersion { found, min } => {
            assert_eq!(found, "1.10");
            assert_eq!(min, "1.22");
        }
        other => panic!("expected OutdatedVersion, got {other:?}"),
    }
}
