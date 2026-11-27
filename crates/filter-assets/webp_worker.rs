use super::{
    fit_within_filter_limits, shrink_for_filter, shrink_target, sniff_image_dimensions,
    ShrinkError, ShrunkImage, FILTER_MAX_IMAGE_BYTES, MAX_SHRINK_WORKING_SET,
};
use image::ImageFormat;
use l4a_logging::process::CommandAudit;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Semaphore};

const WORKER_ARG: &str = "--l4a-filter-webp-worker";
const WORKER_TIMEOUT: Duration = Duration::from_secs(20);
static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();

/// Dispatch the internal worker before starting the app, or register the current executable.
pub fn init_filter_image_worker() {
    if std::env::args_os().nth(1).as_deref() == Some(WORKER_ARG.as_ref()) {
        if let Err(err) = worker_main() {
            eprintln!("{err}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    if let Ok(executable) = std::env::current_exe() {
        let _ = EXECUTABLE.set(executable);
    }
}

pub(super) async fn resize(
    bytes: Vec<u8>,
    source_size: (u32, u32),
    permits: &'static Semaphore,
) -> Result<ShrunkImage, ShrinkError> {
    if bytes.len() > FILTER_MAX_IMAGE_BYTES {
        return Err(ShrinkError::Decode("WebP 源图超出体积限制".into()));
    }
    let executable = EXECUTABLE
        .get()
        .ok_or_else(|| ShrinkError::Decode("WebP 缩放进程未初始化".into()))?;
    let permit = permits.acquire().await.map_err(decode_error)?;
    let (_request, canceled) = oneshot::channel();
    tokio::spawn(async move {
        // Keep the permit through termination and reaping after request cancellation.
        let _permit = permit;
        run_worker(executable, bytes, source_size, canceled).await
    })
    .await
    .map_err(decode_error)?
}

async fn run_worker(
    executable: &Path,
    bytes: Vec<u8>,
    source_size: (u32, u32),
    canceled: oneshot::Receiver<()>,
) -> Result<ShrunkImage, ShrinkError> {
    let mut command = Command::new(executable);
    command
        .arg(WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);

    let audit = CommandAudit::start_quiet("filter.image.resize.webp", command.as_std());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            audit.finish_error(&err);
            return Err(decode_error(err));
        }
    };
    let result = tokio::select! {
        result = tokio::time::timeout(WORKER_TIMEOUT, exchange(&mut child, &bytes)) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "WebP 缩放超时"))
                .and_then(|result| result)
        }
        _ = canceled => Err(io::Error::new(io::ErrorKind::Interrupted, "WebP 缩放已取消")),
    };
    let (status, bytes) = match result {
        Ok((status, bytes)) => {
            audit.finish_status(&Ok(status));
            (status, bytes)
        }
        Err(err) => {
            let _ = child.kill().await;
            audit.finish_error(&err);
            return Err(decode_error(err));
        }
    };
    if !status.success() {
        return Err(ShrinkError::Decode(format!("WebP 缩放进程退出 ({status})")));
    }
    let (w, h, format) = sniff_image_dimensions(&bytes).map_err(decode_error)?;
    if bytes.len() > FILTER_MAX_IMAGE_BYTES
        || format != ImageFormat::Jpeg
        || w == 0
        || h == 0
        || fit_within_filter_limits(w, h).is_some()
    {
        return Err(ShrinkError::Decode("WebP 缩放结果超出图片限制".into()));
    }
    Ok(ShrunkImage {
        bytes,
        source_size,
        output_size: (w, h),
    })
}

async fn exchange(child: &mut Child, bytes: &[u8]) -> io::Result<(ExitStatus, Vec<u8>)> {
    let mut stdin = child.stdin.take().expect("worker stdin is piped");
    let mut stdout = child
        .stdout
        .take()
        .expect("worker stdout is piped")
        .take((FILTER_MAX_IMAGE_BYTES + 1) as u64);
    let mut jpeg = Vec::new();
    let ((), _, status) = tokio::try_join!(
        async move { stdin.write_all(bytes).await },
        stdout.read_to_end(&mut jpeg),
        child.wait(),
    )?;
    Ok((status, jpeg))
}

fn worker_main() -> Result<(), ShrinkError> {
    limit_process_memory().map_err(decode_error)?;
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take((FILTER_MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(decode_error)?;
    if bytes.len() > FILTER_MAX_IMAGE_BYTES {
        return Err(ShrinkError::Decode("WebP 源图超出体积限制".into()));
    }
    let (w, h, format) = sniff_image_dimensions(&bytes).map_err(decode_error)?;
    if format != ImageFormat::WebP {
        return Err(ShrinkError::Decode("缩放进程仅接受 WebP 图片".into()));
    }
    let target_size =
        shrink_target(w, h)?.ok_or_else(|| ShrinkError::Decode("WebP 图片无需缩放".into()))?;
    let shrunk = shrink_for_filter(&bytes, format, (w, h), target_size)?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&shrunk.bytes).map_err(decode_error)?;
    stdout.flush().map_err(decode_error)
}

fn decode_error(err: impl std::fmt::Display) -> ShrinkError {
    ShrinkError::Decode(err.to_string())
}

#[cfg(unix)]
fn limit_process_memory() -> io::Result<()> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: Both system calls receive a valid rlimit pointer. The worker keeps any
    // stricter inherited limit and applies the cap before reading image data.
    unsafe {
        if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::getrlimit(libc::RLIMIT_AS, &mut limit) != 0 {
            return Err(io::Error::last_os_error());
        }
        limit.rlim_cur = limit.rlim_cur.min(MAX_SHRINK_WORKING_SET as libc::rlim_t);
        limit.rlim_max = limit.rlim_cur;
        if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn limit_process_memory() -> io::Result<()> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::{JobObjects::*, Threading::GetCurrentProcess};

    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.ProcessMemoryLimit = MAX_SHRINK_WORKING_SET as usize;
    // SAFETY: The job is anonymous, and the limit structure and its length match the
    // information class. The job remains active for the associated process after close.
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let result = if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        ) == 0
            || AssignProcessToJobObject(job, GetCurrentProcess()) == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        CloseHandle(job);
        result
    }
}

#[cfg(not(any(unix, windows)))]
fn limit_process_memory() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "此平台无法限制 WebP 缩放进程的内存",
    ))
}
