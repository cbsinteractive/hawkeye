use crate::video_stream;
use lazy_static::lazy_static;
use log::debug;
use prometheus::{self, Encoder, TextEncoder};
use prometheus::{register_histogram, register_int_counter, Histogram, IntCounter};
use tokio::runtime::Builder;
use warp::hyper::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use warp::hyper::{Body, StatusCode};
use warp::reply::Response;
use warp::Filter;

lazy_static! {
    pub static ref FOUND_SLATE_COUNTER: IntCounter = register_int_counter!(
        "slate_found_in_stream",
        "Number of times a slate image was found in the stream"
    )
    .unwrap();
    pub static ref FOUND_CONTENT_COUNTER: IntCounter = register_int_counter!(
        "content_found_in_stream",
        "Number of times the content was found in the stream"
    )
    .unwrap();
    pub static ref SIMILARITY_EXECUTION_COUNTER: IntCounter = register_int_counter!(
        "similarity_execution",
        "Number of times we searched for slate in the stream"
    )
    .unwrap();
    pub static ref SIMILARITY_EXECUTION_DURATION: Histogram = register_histogram!(
        "similarity_execution_seconds",
        "Seconds it took to execute the similarity algorithm"
    )
    .unwrap();
    pub static ref FRAME_PROCESSING_DURATION: Histogram = register_histogram!(
        "frame_processing_seconds",
        "Seconds it took to execute the whole frame processing block"
    )
    .unwrap();
    pub static ref HTTP_CALL_DURATION: Histogram = register_histogram!(
        "http_call_action_execution_seconds",
        "Seconds it took to execute the HTTP call"
    )
    .unwrap();
    pub static ref HTTP_CALL_SUCCESS_COUNTER: IntCounter = register_int_counter!(
        "http_call_success",
        "Number of times the HTTP call executed successfully"
    )
    .unwrap();
    pub static ref HTTP_CALL_ERROR_COUNTER: IntCounter = register_int_counter!(
        "http_call_error",
        "Number of times the HTTP call returned an HTTP error status code"
    )
    .unwrap();
    pub static ref HTTP_CALL_RETRIED_COUNT: IntCounter = register_int_counter!(
        "http_call_retried",
        "Number of times the HTTP call was retried"
    )
    .unwrap();
    pub static ref HTTP_CALL_RETRIES_EXHAUSTED_COUNT: IntCounter = register_int_counter!(
        "http_call_retries_exhausted",
        "Number of times the HTTP action has exhausted all the retries"
    )
    .unwrap();
}

fn get_metric_contents() -> String {
    debug!("Metrics endpoint called!");
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    String::from_utf8(buffer).unwrap()
}

fn latest_frame() -> impl warp::Reply {
    let image = video_stream::LATEST_FRAME.read();
    let image_png = HeaderValue::from_static("image/png");
    let no_store = HeaderValue::from_static("no-store");
    let response = match &*image {
        Some(image) => {
            let mut res = Response::new(image.clone().into());
            let headers = res.headers_mut();
            headers.insert(CONTENT_TYPE, image_png);
            headers.insert(CACHE_CONTROL, no_store);
            res
        }
        None => {
            let mut res = Response::new(Body::empty());
            let headers = res.headers_mut();
            headers.insert(CONTENT_TYPE, image_png);
            headers.insert(CACHE_CONTROL, no_store);
            let status = res.status_mut();
            *status = StatusCode::NOT_FOUND;
            res
        }
    };
    Ok(response)
}

pub fn run_metrics_service() {
    let mut runtime = Builder::new()
        .threaded_scheduler()
        .thread_name("metrics_app")
        .max_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let routes = warp::get().and(
        warp::path("metrics")
            .map(get_metric_contents)
            .or(warp::path("latest_frame").map(latest_frame)),
    );
    runtime.block_on(warp::serve(routes).run(([0, 0, 0, 0], 3030)));
}
