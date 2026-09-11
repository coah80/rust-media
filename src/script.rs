use boa_engine::{Context, Script, Source};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::{Context as TaskContext, Poll, Waker},
    time::{Duration, Instant},
};

static RUNNING: AtomicBool = AtomicBool::new(false);
struct ResolverSlot;
impl Drop for ResolverSlot {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

pub fn transform(script: String, input: String, cancel: Arc<AtomicBool>) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Video loading cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("YouTube resolver is busy, try again".into());
        }
        if RUNNING
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("youtube-script".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let _slot = ResolverSlot;
            let result = run(&script, &input, &worker_cancel);
            let _ = sender.send(result);
        })
        .map_err(|_| {
            RUNNING.store(false, Ordering::Release);
            "Could not start the YouTube resolver"
        })?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Video loading cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("YouTube URL resolution timed out".into());
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("The YouTube resolver failed".into());
            }
        }
    }
}

fn run(source: &str, input: &str, cancel: &AtomicBool) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let code =
        ytdlp_ejs::preprocess_player(source).map_err(|_| "YouTube changed its player script")?;
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(1_000_000);
    context.runtime_limits_mut().set_recursion_limit(256);
    let code = format!(
        "var _result = {{}};\n{code}\n_result.n({});",
        serde_json::to_string(input).map_err(|_| "Invalid stream parameter")?
    );
    let output = evaluate(&code, &mut context, cancel, deadline)?;
    if output == input {
        return Err("YouTube URL resolution failed".into());
    }
    Ok(output)
}

fn evaluate(
    code: &str,
    context: &mut Context,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<String, String> {
    let script = Script::parse(Source::from_bytes(code), None, context)
        .map_err(|_| "Could not parse the YouTube player")?;
    let mut future = std::pin::pin!(script.evaluate_async_with_budget(context, 10_000));
    let mut task = TaskContext::from_waker(Waker::noop());
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Video loading cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("YouTube URL resolution timed out".into());
        }
        match future.as_mut().poll(&mut task) {
            Poll::Pending => {}
            Poll::Ready(result) => {
                let value = result.map_err(|_| "Could not resolve the YouTube stream URL")?;
                let value = value
                    .as_string()
                    .ok_or("Invalid YouTube stream parameter")?
                    .to_std_string_escaped();
                if value.is_empty() || value.len() > 4096 || value.starts_with("enhanced_except_") {
                    return Err("YouTube URL resolution failed".into());
                }
                return Ok(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripts_have_no_host_io_and_can_be_cancelled() {
        let mut context = Context::default();
        let cancel = AtomicBool::new(false);
        let result = evaluate(
            "[typeof fetch, typeof process, typeof require].join(',')",
            &mut context,
            &cancel,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(result, "undefined,undefined,undefined");
        cancel.store(true, Ordering::Relaxed);
        assert!(
            evaluate(
                "while(true){}",
                &mut context,
                &cancel,
                Instant::now() + Duration::from_secs(1)
            )
            .unwrap_err()
            .contains("cancelled")
        );
    }
    #[test]
    fn script_execution_obeys_deadline() {
        let mut context = Context::default();
        let began = Instant::now();
        let error = evaluate(
            "while(true){}",
            &mut context,
            &AtomicBool::new(false),
            began + Duration::from_millis(20),
        )
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(began.elapsed() < Duration::from_secs(1));
    }
}
