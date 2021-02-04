use crate::{auth, handlers};
use hawkeye_core::models::Watcher;
use kube::Client;
use serde::Serialize;
use warp::hyper::StatusCode;
use warp::Filter;

/// API root for v1
pub fn v1(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = std::convert::Infallible> + Clone {
    watchers_list(client.clone())
        .or(watcher_create(client.clone()))
        .or(watcher_get(client.clone()))
        .or(watcher_delete(client.clone()))
        .or(watcher_start(client.clone()))
        .or(watcher_stop(client.clone()))
        .or(watcher_video_frame(client.clone()))
        .or(healthcheck(client))
        .recover(handle_rejection)
}

/// GET /v1/watchers
pub fn watchers_list(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers")
        .and(auth::verify())
        .and(warp::get())
        .and(with_client(client))
        .and_then(handlers::list_watchers)
}

/// POST /v1/watchers
pub fn watcher_create(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers")
        .and(auth::verify())
        .and(warp::post())
        .and(json_body())
        .and(with_client(client))
        .and_then(handlers::create_watcher)
}

/// GET /v1/watchers/{id}
pub fn watcher_get(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers" / String)
        .and(auth::verify())
        .and(warp::get())
        .and(with_client(client))
        .and_then(handlers::get_watcher)
}

/// DELETE /v1/watchers/{id}
pub fn watcher_delete(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers" / String)
        .and(auth::verify())
        .and(warp::delete())
        .and(with_client(client))
        .and_then(handlers::delete_watcher)
}

/// POST /v1/watchers/{id}/start
pub fn watcher_start(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers" / String / "start")
        .and(auth::verify())
        .and(warp::post())
        .and(with_client(client))
        .and_then(handlers::start_watcher)
}

/// POST /v1/watchers/{id}/stop
pub fn watcher_stop(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers" / String / "stop")
        .and(auth::verify())
        .and(warp::post())
        .and(with_client(client))
        .and_then(handlers::stop_watcher)
}

/// GET /v1/watchers/{id}/video-frame
pub fn watcher_video_frame(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("v1" / "watchers" / String / "video-frame")
        .and(warp::get())
        .and(with_client(client))
        .and_then(handlers::get_video_frame)
}

/// GET /healthcheck
pub fn healthcheck(
    client: Client,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path("healthcheck")
        .and(warp::get())
        .and(with_client(client))
        .and_then(handlers::healthcheck)
}

fn with_client(
    client: Client,
) -> impl Filter<Extract = (Client,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || client.clone())
}

fn json_body() -> impl Filter<Extract = (Watcher,), Error = warp::Rejection> + Clone {
    // When accepting a body, we want a JSON body
    // (and to reject huge payloads)...
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

/// An API error serializable to JSON.
#[derive(Serialize)]
struct ErrorMessage {
    message: String,
}

async fn handle_rejection(
    err: warp::Rejection,
) -> Result<impl warp::Reply, std::convert::Infallible> {
    let message = "Error calling the API".to_string();
    let code;

    log::debug!("Rejection = {:?}", err);

    if err.is_not_found() {
        code = StatusCode::NOT_FOUND;
    } else if err.find::<auth::NoAuth>().is_some() {
        code = StatusCode::UNAUTHORIZED;
    } else if let Some(missing) = err.find::<warp::reject::MissingHeader>() {
        if missing.name() == "authorization" {
            code = StatusCode::UNAUTHORIZED;
        } else {
            code = StatusCode::BAD_REQUEST;
        }
    } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
        code = StatusCode::METHOD_NOT_ALLOWED;
    } else {
        log::debug!("Unhandled rejection: {:?}", err);
        code = StatusCode::INTERNAL_SERVER_ERROR;
    }

    let json = warp::reply::json(&ErrorMessage { message });
    Ok(warp::reply::with_status(json, code))
}
