use crate::metrics::{
    HTTP_CALL_DURATION, HTTP_CALL_ERROR_COUNTER, HTTP_CALL_RETRIED_COUNT,
    HTTP_CALL_RETRIES_EXHAUSTED_COUNT, HTTP_CALL_SUCCESS_COUNTER,
};
use crate::video_stream::Event;
use color_eyre::Result;
use crossbeam::channel::Receiver;
use hawkeye_core::models::{self, Action, HttpAuth, HttpCall, VideoMode};
use log::{debug, error, info, warn};
use std::time::Duration;

#[cfg(test)]
use sn_fake_clock::FakeClock as Instant;
#[cfg(not(test))]
use std::time::Instant;

/// Abstracts execution call for every action type.
trait ActionExecution {
    fn execute(&mut self) -> Result<()>;
}

impl ActionExecution for Action {
    fn execute(&mut self) -> Result<()> {
        match self {
            Action::HttpCall(a) => a.execute(),
            Action::FakeAction(a) => a.execute(),
        }
    }
}

/// Represents a sequence of video modes.
#[derive(Clone, Eq, PartialEq)]
pub struct Transition(VideoMode, VideoMode);

/// Manages the execution of an `Action` based on a flow of `VideoMode`s.
///
/// The `ActionExecutor` abstracts the logic of execution that is inherent to all `Action` types.
pub struct ActionExecutor {
    transition: Transition,
    action: Action,
    last_mode: Option<VideoMode>,
    last_call: Option<Instant>,
}

impl ActionExecutor {
    /// Creates a new `ActionExecutor` instance
    pub fn new(transition: Transition, action: Action) -> Self {
        Self {
            transition,
            action,
            last_mode: None,
            last_call: None,
        }
    }

    // Manage the execution of an action based on the provided video mode.
    pub fn execute(&mut self, mode: VideoMode) {
        if let Some(result) = self.call_action(mode) {
            match result {
                Ok(_) => self.last_call = Some(Instant::now()),
                Err(err) => error!(
                    "Error while processing action in mode {:?}: {:#}",
                    mode, err
                ),
            }
        }
        self.last_mode = Some(mode);
    }

    /// Executes the action if the video mode matches the transition and if the action is
    /// allowed to run.
    fn call_action(&mut self, mode: VideoMode) -> Option<Result<()>> {
        self.last_mode.and_then(|last_mode| {
            if Transition(last_mode, mode) == self.transition && self.allowed_to_run() {
                Some(self.action.execute())
            } else {
                None
            }
        })
    }

    /// Check if the action is allowed to run within the timeframe it was called.
    ///
    /// We need to limit the action frequency since the source of video mode does not guarantee the
    /// ordering of events.
    fn allowed_to_run(&self) -> bool {
        match &self.last_call {
            None => true,
            Some(last_call) => last_call.elapsed() > Duration::from_secs(5),
        }
    }
}

// TODO: Delete this type
pub(crate) struct Executors(pub(crate) Vec<ActionExecutor>);

impl From<models::Transition> for Executors {
    fn from(transition: models::Transition) -> Self {
        let target_transition = Transition(transition.from, transition.to);
        Self(
            transition
                .actions
                .into_iter()
                .map(|action| ActionExecutor::new(target_transition.clone(), action))
                .collect(),
        )
    }
}

pub struct Runtime {
    receiver: Receiver<Event>,
    actions: Vec<ActionExecutor>,
}

impl Runtime {
    pub fn new(receiver: Receiver<Event>, processors: Vec<ActionExecutor>) -> Self {
        Runtime {
            receiver,
            actions: processors,
        }
    }

    pub fn run_blocking(&mut self) -> Result<()> {
        loop {
            match self.receiver.recv()? {
                Event::Terminate => break,
                Event::Mode(mode) => {
                    for p in self.actions.iter_mut() {
                        p.execute(mode);
                    }
                }
            }
        }
        Ok(())
    }
}

impl ActionExecution for HttpCall {
    fn execute(&mut self) -> Result<()> {
        let mut tries = 0;
        loop {
            match try_call(&self) {
                Ok(_) => break,
                Err(err) => {
                    HTTP_CALL_RETRIED_COUNT.inc();
                    tries += 1;
                    if tries >= self.retries.unwrap_or(0) {
                        HTTP_CALL_RETRIES_EXHAUSTED_COUNT.inc();
                        return Err(err);
                    }
                }
            }
        }
        Ok(())
    }
}

fn try_call(call: &HttpCall) -> Result<()> {
    let timer = HTTP_CALL_DURATION.start_timer();
    let method = call.method.to_string();
    let mut request = ureq::request(&method, call.url.as_str());

    request.timeout_connect(500);

    if let Some(HttpAuth::Basic { username, password }) = &call.authorization {
        request.auth(username, password);
    }

    if let Some(timeout) = &call.timeout {
        request.timeout(Duration::from_secs(*timeout as u64));
    }

    if let Some(headers) = &call.headers {
        for (k, v) in headers.iter() {
            request.set(k, v);
        }
    }

    let response = match call.body.as_ref() {
        Some(data) => request.send_string(data),
        None => request.call(),
    };
    if response.ok() {
        HTTP_CALL_SUCCESS_COUNTER.inc();
        debug!(
            "Successfully called backend API {}",
            response.into_string()?
        );
    } else {
        HTTP_CALL_ERROR_COUNTER.inc();
        warn!(
            "Error while calling backend API ({}): {}",
            response.status(),
            response.into_string()?
        );
    }

    // Report how long it took to call the backend.
    // Keep it out of the log macro, so it will execute every time independent of log level
    let seconds = timer.stop_and_record();
    info!(
        "HTTP call to backend API took: {}ms",
        Duration::from_secs_f64(seconds).as_millis()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam::channel::unbounded;
    use hawkeye_core::models::{FakeAction, HttpMethod};
    use mockito::{mock, server_url, Matcher};
    use sn_fake_clock::FakeClock;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn sleep(d: Duration) {
        FakeClock::advance_time(d.as_millis() as u64);
    }

    #[test]
    fn executor_slate_action_called_when_transition_content_to_slate() {
        let called = Arc::new(AtomicBool::new(false));
        let fake_action = FakeAction {
            called: called.clone(),
            execute_returns: Some(Ok(())),
        };
        let mut executor = ActionExecutor::new(
            Transition(VideoMode::Content, VideoMode::Slate),
            Action::FakeAction(fake_action),
        );
        executor.execute(VideoMode::Content);
        // Didn't call since it was the first state found
        assert_eq!(called.load(Ordering::SeqCst), false);

        executor.execute(VideoMode::Slate);
        // Must be called since we had a state transition that matches what we defined in the executor
        assert_eq!(called.load(Ordering::SeqCst), true);
    }

    #[test]
    fn executor_slate_action_cannot_be_called_twice_in_short_timeframe() {
        let called = Arc::new(AtomicBool::new(false));
        let fake_action = FakeAction {
            called: called.clone(),
            execute_returns: Some(Ok(())),
        };
        let mut executor = ActionExecutor::new(
            Transition(VideoMode::Content, VideoMode::Slate),
            Action::FakeAction(fake_action),
        );
        executor.execute(VideoMode::Content);
        executor.execute(VideoMode::Slate);
        // Must be called since we had a state transition that matches what we defined in the executor
        assert_eq!(called.load(Ordering::SeqCst), true);
        // Reset state of our mock to "not called"
        called.store(false, Ordering::SeqCst);
        executor.execute(VideoMode::Content);
        executor.execute(VideoMode::Slate);
        assert_eq!(called.load(Ordering::SeqCst), false);
    }

    #[test]
    fn executor_slate_action_can_be_called_twice_after_some_time_passes() {
        let called = Arc::new(AtomicBool::new(false));
        let fake_action = FakeAction {
            called: called.clone(),
            execute_returns: Some(Ok(())),
        };
        let mut executor = ActionExecutor::new(
            Transition(VideoMode::Content, VideoMode::Slate),
            Action::FakeAction(fake_action),
        );
        executor.execute(VideoMode::Content);
        executor.execute(VideoMode::Slate);
        // Must be called since we had a state transition that matches what we defined in the executor
        assert_eq!(called.load(Ordering::SeqCst), true);
        // Reset state of our mock to "not called"
        called.store(false, Ordering::SeqCst);

        // Move time forward over the delay
        sleep(Duration::from_secs(11));

        executor.execute(VideoMode::Content);
        executor.execute(VideoMode::Slate);
        assert_eq!(called.load(Ordering::SeqCst), true);
    }

    #[test]
    fn executor_slate_action_cannot_be_called_twice_if_no_mode_change() {
        let called = Arc::new(AtomicBool::new(false));
        let fake_action = FakeAction {
            called: called.clone(),
            execute_returns: Some(Ok(())),
        };
        let mut executor = ActionExecutor::new(
            Transition(VideoMode::Content, VideoMode::Slate),
            Action::FakeAction(fake_action),
        );
        executor.execute(VideoMode::Content);
        executor.execute(VideoMode::Slate);
        // Must be called since we had a state transition that matches what we defined in the executor
        assert_eq!(called.load(Ordering::SeqCst), true);
        // Reset state of our mock to "not called"
        called.store(false, Ordering::SeqCst);

        // Move time forward over the delay
        sleep(Duration::from_secs(20));

        executor.execute(VideoMode::Slate);
        assert_eq!(called.load(Ordering::SeqCst), false);
    }

    #[test]
    fn runtime_calls_action_executor_with_video_mode() {
        let called = Arc::new(AtomicBool::new(false));
        let fake_action = FakeAction {
            called: called.clone(),
            execute_returns: Some(Ok(())),
        };
        let mut executor = ActionExecutor::new(
            Transition(VideoMode::Content, VideoMode::Slate),
            Action::FakeAction(fake_action),
        );
        // Prepare executor to be ready in the next call with `VideoMode::Slate`
        executor.execute(VideoMode::Content);
        assert_eq!(called.load(Ordering::SeqCst), false);

        let (s, r) = unbounded();
        // Pile up some events for the runtime to consume
        s.send(Event::Mode(VideoMode::Slate)).unwrap();
        s.send(Event::Terminate).unwrap();

        let mut runtime = Runtime::new(r, vec![executor]);
        runtime.run_blocking().expect("Should run successfully!");

        // Check the action was called
        assert_eq!(called.load(Ordering::SeqCst), true);
    }

    #[test]
    fn action_http_call_performs_request() {
        let path = "/do-something";
        let req_body = "{\"duration\":20}";

        let server = mock("POST", path)
            .match_body(req_body)
            .match_header("content-type", "application/json")
            .match_header("authorization", Matcher::Any)
            .with_status(202)
            .create();

        let mut action = HttpCall {
            method: HttpMethod::POST,
            url: format!("{}{}", server_url(), path),
            description: None,
            authorization: Some(HttpAuth::Basic {
                username: "user".to_string(),
                password: "pass".to_string(),
            }),
            headers: Some(
                [("content-type", "application/json")]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<HashMap<String, String>>(),
            ),
            body: Some(req_body.to_string()),
            retries: None,
            timeout: None,
        };

        action.execute().expect("Should execute successfully!");
        assert!(server.matched());
    }

    #[test]
    fn build_executor_from_models() {
        let transition = models::Transition {
            from: models::VideoMode::Content,
            to: models::VideoMode::Slate,
            actions: vec![models::Action::HttpCall(HttpCall {
                description: Some("Trigger AdBreak using API".to_string()),
                method: HttpMethod::POST,
                url: "http://non-existent.cbsi.com/v1/organization/cbsa/channel/sl/ad-break"
                    .to_string(),
                authorization: Some(HttpAuth::Basic {
                    username: "dev_user".to_string(),
                    password: "something".to_string(),
                }),
                headers: Some(
                    [("content-type", "application/json")]
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect::<HashMap<String, String>>(),
                ),
                body: Some("{\"duration\":320}".to_string()),
                retries: Some(3),
                timeout: Some(10),
            })],
        };

        let _executors: Executors = transition.into();
    }
}
