//! preflight: gh 미설치 / 미인증 / 구버전 / ready 구분(플랜 §3.2).

mod fixture;

use fixture::{activate_no_gh, env_lock, FakeGh};
use suaegi_forge::{preflight, GhRunner, Preflight};

#[tokio::test]
async fn preflight_ready_when_installed_and_authed() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule("--version", "gh version 2.40.0 (2024-01-01)\n", "", 0)
        .rule("auth status", "", "Logged in to github.com as octocat\n", 0);
    let _p = fake.activate();
    assert_eq!(preflight(&GhRunner::new()).await, Preflight::Ready);
}

#[tokio::test]
async fn preflight_not_installed_when_gh_absent() {
    let _g = env_lock();
    let _p = activate_no_gh();
    assert_eq!(preflight(&GhRunner::new()).await, Preflight::NotInstalled);
}

#[tokio::test]
async fn preflight_not_authenticated_when_auth_status_fails() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule("--version", "gh version 2.40.0 (2024-01-01)\n", "", 0)
        .rule(
            "auth status",
            "",
            "You are not logged into any GitHub hosts. Run gh auth login to authenticate.\n",
            1,
        );
    let _p = fake.activate();
    assert_eq!(
        preflight(&GhRunner::new()).await,
        Preflight::NotAuthenticated
    );
}

#[tokio::test]
async fn preflight_outdated_version_is_flagged() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule("--version", "gh version 1.14.0 (2021-01-01)\n", "", 0)
        .rule("auth status", "", "Logged in\n", 0);
    let _p = fake.activate();
    match preflight(&GhRunner::new()).await {
        Preflight::OutdatedVersion { found, min } => {
            assert_eq!(found, "1.14");
            assert_eq!(min, "2.0");
        }
        other => panic!("expected OutdatedVersion, got {other:?}"),
    }
}
