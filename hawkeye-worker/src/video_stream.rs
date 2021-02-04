use crate::img_detector::SlateDetector;
use crate::metrics::{
    FOUND_CONTENT_COUNTER, FOUND_SLATE_COUNTER, FRAME_PROCESSING_DURATION,
    SIMILARITY_EXECUTION_COUNTER, SIMILARITY_EXECUTION_DURATION,
};
use crate::slate::SLATE_SIZE;
use color_eyre::Result;
use concread::CowCell;
use crossbeam::channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use derive_more::{Display, Error};
use gst::gst_element_error;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use hawkeye_core::models::{Codec, Container, VideoMode};
use lazy_static::lazy_static;
use log::{debug, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

lazy_static! {
    pub(crate) static ref LATEST_FRAME: CowCell<Option<Vec<u8>>> = CowCell::new(None);
}

#[derive(Debug, Display, Error)]
#[display(fmt = "Received error from {}: {} (debug: {:?})", src, error, debug)]
struct ErrorMessage {
    src: String,
    error: String,
    debug: Option<String>,
    source: glib::Error,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Terminate,
    Mode(VideoMode),
}

pub fn process_frames(
    frame_source: impl Iterator<Item = Result<Option<Vec<u8>>>>,
    detector: SlateDetector,
    running: Arc<AtomicBool>,
    action_sink: Sender<Event>,
) -> Result<()> {
    let black_image = include_bytes!("../../resources/black_120px.jpg");
    let black_detector = SlateDetector::new(black_image)?;

    let mut empty_iterations = 0;
    for frame in frame_source {
        let frame_processing_timer = FRAME_PROCESSING_DURATION.start_timer();
        let local_buffer = match frame? {
            Some(contents) => {
                log::trace!("Empty iterations: {}", empty_iterations);
                empty_iterations = 0;
                contents
            }
            None => {
                if !running.load(Ordering::SeqCst) {
                    break;
                } else {
                    empty_iterations += 1;
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        };

        let is_black = black_detector.is_match(local_buffer.as_slice());

        let mut is_match = false;
        if !is_black {
            let t = SIMILARITY_EXECUTION_DURATION.start_timer();

            is_match = detector.is_match(local_buffer.as_slice());

            let took_in_seconds = t.stop_and_record();
            log::trace!("Similarity algorithm ran in {} seconds", took_in_seconds);
        }

        {
            // Save latest image bytes
            let mut write_txn = LATEST_FRAME.write();
            // Moves the local buffer
            *write_txn = Some(local_buffer);
            write_txn.commit();
        }

        if is_black {
            continue;
        }

        if is_match {
            log::trace!("Found slate image in video stream!");
            FOUND_SLATE_COUNTER.inc();
            action_sink.send(Event::Mode(VideoMode::Slate)).unwrap();
        } else {
            FOUND_CONTENT_COUNTER.inc();
            action_sink.send(Event::Mode(VideoMode::Content)).unwrap();
            log::trace!("Content in video stream!");
        }
        SIMILARITY_EXECUTION_COUNTER.inc();

        let took_in_seconds = frame_processing_timer.stop_and_record();
        log::trace!("Frame processing took {} seconds", took_in_seconds);
        if !running.load(Ordering::SeqCst) {
            break;
        }
    }

    info!("Stopping pipeline gracefully!");
    action_sink.send(Event::Terminate)?;

    Ok(())
}

pub struct RtpServer {
    ingest_port: u32,
    container: Container,
    codec: Codec,
}

impl RtpServer {
    pub fn new(ingest_port: u32, container: Container, codec: Codec) -> Self {
        Self {
            ingest_port,
            container,
            codec,
        }
    }
}

impl IntoIterator for RtpServer {
    type Item = Result<Option<Vec<u8>>>;
    type IntoIter = VideoStreamIterator;

    fn into_iter(self) -> Self::IntoIter {
        let (width, height) = SLATE_SIZE;
        let pipeline_description = match (self.container, self.codec) {
            (Container::MpegTs, Codec::H264) => format!(
                "udpsrc port={} caps=\"application/x-rtp, media=(string)video, clock-rate=(int)90000, encoding-name=(string)MP2T, payload=(int)33\" ! .recv_rtp_sink_0 rtpbin ! rtpmp2tdepay ! tsdemux ! h264parse ! avdec_h264 ! videorate ! video/x-raw,framerate=10/1 ! videoconvert ! videoscale ! capsfilter caps=\"video/x-raw, width={}, height={}\"",
                self.ingest_port,
                width,
                height
            ),
            (Container::RawVideo, Codec::H264) => format!(
                "udpsrc port={} caps=\"application/x-rtp, media=(string)video, clock-rate=(int)90000, encoding-name=(string)H264, payload=(int)96\" ! rtph264depay ! decodebin ! videorate ! video/x-raw,framerate=10/1 ! videoconvert ! videoscale ! capsfilter caps=\"video/x-raw, width={}, height={}\"",
                self.ingest_port,
                width,
                height
            ),
            (_, _) => {
                panic!(format!("Container ({:?}) and Codec ({:?}) not available", self.container, self.codec));
            }
        };
        VideoStream::new(pipeline_description).into_iter()
    }
}

pub struct VideoStream {
    pipeline_description: String,
}

impl VideoStream {
    pub fn new<S: AsRef<str>>(pipeline_description: S) -> Self {
        Self {
            pipeline_description: String::from(pipeline_description.as_ref()),
        }
    }
}

impl IntoIterator for VideoStream {
    type Item = Result<Option<Vec<u8>>>;
    type IntoIter = VideoStreamIterator;

    fn into_iter(self) -> Self::IntoIter {
        let (sender, receiver) = bounded(1);

        debug!("Creating GStreamer Pipeline..");
        let pipeline = gst::parse_launch(
            format!(
                "{} ! pngenc snapshot=false ! appsink name=sink",
                self.pipeline_description
            )
            .as_str(),
        )
        .expect("Pipeline description invalid, cannot create")
        .downcast::<gst::Pipeline>()
        .expect("Expected a gst::Pipeline");

        // Get access to the appsink element.
        let appsink = pipeline
            .get_by_name("sink")
            .expect("Sink element not found")
            .downcast::<gst_app::AppSink>()
            .expect("Sink element is expected to be an appsink!");

        appsink
            .set_property("sync", &false)
            .expect("Failed to disable gst pipeline sync");
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    // Pull the sample in question out of the appsink's buffer.
                    let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer_ref = sample.get_buffer().ok_or_else(|| {
                        gst_element_error!(
                            appsink,
                            gst::ResourceError::Failed,
                            ("Failed to get buffer from appsink")
                        );

                        if let Err(err) = sender.try_send(Err(color_eyre::eyre::eyre!(
                            "Failed to get buffer from appsink"
                        ))) {
                            log::error!("Could not send message in stream: {}", err)
                        }

                        gst::FlowError::Error
                    })?;

                    // At this point, buffer is only a reference to an existing memory region somewhere.
                    // When we want to access its content, we have to map it while requesting the required
                    // mode of access (read, read/write).
                    // This type of abstraction is necessary, because the buffer in question might not be
                    // on the machine's main memory itself, but rather in the GPU's memory.
                    // So mapping the buffer makes the underlying memory region accessible to us.
                    // See: https://gstreamer.freedesktop.org/documentation/plugin-development/advanced/allocation.html
                    let buffer = buffer_ref.map_readable().map_err(|_| {
                        gst_element_error!(
                            appsink,
                            gst::ResourceError::Failed,
                            ("Failed to map buffer readable")
                        );

                        if let Err(err) = sender.try_send(Err(color_eyre::eyre::eyre!(
                            "Failed to map buffer readable"
                        ))) {
                            log::error!("Could not send message in stream: {}", err)
                        }

                        gst::FlowError::Error
                    })?;
                    log::trace!("Frame extracted from pipeline");

                    match sender.try_send(Ok(Some(buffer.to_vec()))) {
                        Ok(_) => Ok(gst::FlowSuccess::Ok),
                        Err(TrySendError::Full(_)) => {
                            log::trace!("Channel is full, discarded frame");
                            Ok(gst::FlowSuccess::Ok)
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            log::debug!("Returning EOS in pipeline callback fn");
                            Err(gst::FlowError::Eos)
                        }
                    }
                })
                .build(),
        );

        let bus = pipeline
            .get_bus()
            .expect("Pipeline without bus. Shouldn't happen!");

        pipeline
            .set_state(gst::State::Playing)
            .expect("Cannot start pipeline");
        info!("Pipeline started: {}", self.pipeline_description);

        VideoStreamIterator {
            description: self.pipeline_description,
            receiver,
            pipeline,
            bus,
        }
    }
}

pub struct VideoStreamIterator {
    description: String,
    receiver: Receiver<Result<Option<Vec<u8>>>>,
    pipeline: gst::Pipeline,
    bus: gst::Bus,
}

impl Iterator for VideoStreamIterator {
    type Item = Result<Option<Vec<u8>>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.receiver.try_recv() {
            Ok(event) => return Some(event),
            Err(TryRecvError::Empty) => {
                // Check if there are errors in the GStreamer pipeline itself.
                if let Some(msg) = self.bus.pop() {
                    use gst::MessageView;

                    match msg.view() {
                        MessageView::Eos(..) => {
                            // The End-of-stream message is posted when the stream is done, which in our case
                            // happens immediately after matching the slate image because we return
                            // gst::FlowError::Eos then.
                            return None;
                        }
                        MessageView::Error(err) => {
                            let error_msg = ErrorMessage {
                                src: msg
                                    .get_src()
                                    .map(|s| String::from(s.get_path_string()))
                                    .unwrap_or_else(|| String::from("None")),
                                error: err.get_error().to_string(),
                                debug: err.get_debug(),
                                source: err.get_error(),
                            };
                            log::error!("Error returned by pipeline: {:?}", error_msg);
                            // TODO: Should return a proper error here, returning `None` will simply stop the iterator.
                            return None;
                        }
                        _ => (),
                    }
                }
            }
            Err(TryRecvError::Disconnected) => {
                log::debug!("The Pipeline channel is disconnected: {}", self.description);
                return None;
            }
        }
        // Nothing to report in this iteration.
        // Frames could not be captured, but there are no errors in the pipeline.
        Some(Ok(None))
    }
}

impl Drop for VideoStreamIterator {
    fn drop(&mut self) {
        if self.pipeline.set_state(gst::State::Null).is_err() {
            log::error!("Could not stop pipeline");
        }
        log::debug!("Pipeline stopped!");
    }
}
