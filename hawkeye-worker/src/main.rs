mod actions;
mod config;
mod img_detector;
mod metrics;
mod slate;
mod video_stream;

use crate::actions::{ActionExecutor, Executors};
use crate::config::AppConfig;
use crate::img_detector::SlateDetector;
use crate::metrics::run_metrics_service;
use crate::video_stream::{create_pipeline, main_loop};
use color_eyre::Result;
use gstreamer as gst;
use hawkeye_core::models::Watcher;
use log::info;
use pretty_env_logger::env_logger;
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use structopt::StructOpt;

fn main() -> Result<()> {
    color_eyre::install()?;
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let config: AppConfig = AppConfig::from_args();
    let watcher_config = File::open(config.watcher_path)?;
    let watcher: Watcher = serde_json::from_reader(watcher_config)?;
    watcher
        .is_valid()
        .expect("Invalid configuration for Watcher");

    info!("Initializing GStreamer..");
    gst::init().expect("Could not initialize GStreamer!");

    let (sender, receiver) = channel();

    info!("Loading executors..");
    let mut executors: Vec<ActionExecutor> = Vec::new();
    for transition in watcher.transitions.iter() {
        let mut execs: Executors = transition.clone().into();
        executors.append(&mut execs.0);
    }

    thread::spawn(move || {
        let mut runtime = actions::Runtime::new(receiver, executors);

        info!("Starting actions runtime..");
        runtime
            .run_blocking()
            .expect("Actions runtime ended unexpectedly!");
    });

    // starts metrics web app
    thread::spawn(run_metrics_service);

    let running = Arc::new(AtomicBool::new(true));

    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting termination handler");

    log::info!(
        "Starting pipeline at rtp://0.0.0.0:{}",
        watcher.source.ingest_port
    );
    let detector = SlateDetector::new(&mut slate::load_img(watcher.slate_url.as_str())?)?;
    create_pipeline(
        detector,
        watcher.source.ingest_port,
        watcher.source.container,
        watcher.source.codec,
        sender.clone(),
    )
    .and_then(|pipeline| main_loop(pipeline, running, sender))?;

    Ok(())
}
