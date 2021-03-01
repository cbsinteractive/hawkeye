use lazy_static::lazy_static;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use std::iter;

// Environment variable names
const NAMESPACE_ENV: &str = "HAWKEYE_NAMESPACE";
const DOCKER_IMAGE_ENV: &str = "HAWKEYE_DOCKER_IMAGE";
const FIXED_TOKEN_ENV: &str = "HAWKEYE_FIXED_TOKEN";
const CALL_WATCHER_TIMEOUT_ENV: &str = "HAWKEYE_CALL_WATCHER_TIMEOUT_TOKEN";

// Configuration defaults
const DEFAULT_CALL_WATCHER_TIMEOUT: u64 = 2;

lazy_static! {
    /// Kubernetes namespace where the resources are managed (created/deleted/updated)
    pub static ref NAMESPACE: String =
        std::env::var(NAMESPACE_ENV).unwrap_or_else(|_| "default".into());

    /// The docker image of the "hawkeye-worker" to be used in the K8s Deployment resource template
    pub static ref DOCKER_IMAGE: String =
        std::env::var(DOCKER_IMAGE_ENV).unwrap_or_else(|_| "hawkeye-dev:latest".into());

    /// A fixed authentication token required by clients while calling the Hawkeye API
    pub static ref FIXED_TOKEN: String =
        std::env::var(FIXED_TOKEN_ENV).unwrap_or_else(|_| gen_token());

    pub static ref CALL_WATCHER_TIMEOUT: u64 =
        std::env::var(CALL_WATCHER_TIMEOUT_ENV).map(|val| val.parse::<u64>()).unwrap_or_else(|_| Ok(DEFAULT_CALL_WATCHER_TIMEOUT)).unwrap_or(DEFAULT_CALL_WATCHER_TIMEOUT);

}

/// In case the environment variable `HAWKEYE_FIXED_TOKEN` is not present, a
/// random token between 20 and 30 characters is generated. The random token is exposed in a log
/// message for visibility.
fn gen_token() -> String {
    let mut rng = thread_rng();
    let n: usize = rng.gen_range(20, 30);
    let random_token: String = iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .take(n)
        .collect();

    std::env::set_var(FIXED_TOKEN_ENV, &random_token);
    log::info!(
        "Missing security configuration, default token is: {}",
        random_token
    );
    random_token
}
