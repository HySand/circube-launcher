use crate::download_sources::is_bmclapi_url;
use crate::models::{DownloadSource, DownloadSpeedPayload, ProgressPayload};
use futures::{FutureExt, StreamExt};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{Client, StatusCode};
use sha1::{Digest, Sha1};
use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FILE_CONCURRENCY: usize = 64;
const REQUEST_CONCURRENCY: usize = 64;
const BMCLAPI_START_LANES: usize = 2;
const BMCLAPI_REQUEST_INTERVAL: Duration = Duration::from_millis(100);
const NO_DATA_TIMEOUT: Duration = Duration::from_secs(5);
const LOW_SPEED_WINDOW: Duration = Duration::from_secs(5);
const LOW_SPEED_BYTES_PER_SEC: u64 = 1024;
const MAX_ATTEMPTS: usize = 5;

static REQUEST_SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
static BMCLAPI_START_GATES: OnceLock<[tokio::sync::Mutex<Option<Instant>>; BMCLAPI_START_LANES]> =
    OnceLock::new();
static BMCLAPI_NEXT_LANE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub(crate) struct DownloadJob {
    pub(crate) urls: Vec<String>,
    pub(crate) dest: PathBuf,
    pub(crate) sha1: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) check_hash: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorKind {
    Transient,
    RateLimited,
    Integrity,
    Permanent,
}

#[derive(Debug)]
struct DownloadError {
    kind: ErrorKind,
    message: String,
}

impl DownloadError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn retryable(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Transient | ErrorKind::RateLimited | ErrorKind::Integrity
        )
    }
}

impl fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

fn request_semaphore() -> Arc<tokio::sync::Semaphore> {
    REQUEST_SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(REQUEST_CONCURRENCY)))
        .clone()
}

async fn pace_bmclapi_request(url: &str) {
    if !is_bmclapi_url(url) {
        return;
    }

    let gates =
        BMCLAPI_START_GATES.get_or_init(|| std::array::from_fn(|_| tokio::sync::Mutex::new(None)));
    let lane = BMCLAPI_NEXT_LANE.fetch_add(1, Ordering::Relaxed) % BMCLAPI_START_LANES;
    let mut last_started = gates[lane].lock().await;
    if let Some(previous) = *last_started {
        let elapsed = previous.elapsed();
        if elapsed < BMCLAPI_REQUEST_INTERVAL {
            tokio::time::sleep(BMCLAPI_REQUEST_INTERVAL - elapsed).await;
        }
    }
    *last_started = Some(Instant::now());
}

async fn acquire_request(url: &str) -> Result<tokio::sync::OwnedSemaphorePermit, DownloadError> {
    let permit = request_semaphore()
        .acquire_owned()
        .await
        .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
    pace_bmclapi_request(url).await;
    Ok(permit)
}

fn error_for_status(url: &str, status: StatusCode) -> DownloadError {
    let kind = match status.as_u16() {
        403 | 429 => ErrorKind::RateLimited,
        408 | 425 | 500 | 502 | 503 | 504 => ErrorKind::Transient,
        _ => ErrorKind::Permanent,
    };
    DownloadError::new(kind, format!("{} -> HTTP {}", url, status))
}

fn retry_delay(kind: ErrorKind, attempt: usize) -> Duration {
    let exponent = attempt.min(3) as u32;
    match kind {
        ErrorKind::RateLimited => Duration::from_secs(2 * 2u64.pow(exponent)),
        // DNS/connect failures on an OpenBMCLAPI node should return to the
        // BMCLAPI entry point quickly so it can select another node.
        _ => Duration::from_millis(250 * 2u64.pow(exponent)),
    }
}

fn request(client: &Client, url: &str) -> reqwest::RequestBuilder {
    // All generated resources are content-addressed. Let the BMCLAPI node and
    // the OS connection pool reuse them instead of forcing a revalidation on
    // every retry and every parallel piece.
    client.get(url)
}

fn low_speed_window_failed(started_at: &mut Instant, bytes: &mut u64) -> bool {
    let elapsed = started_at.elapsed();
    if elapsed < LOW_SPEED_WINDOW {
        return false;
    }

    let failed = (*bytes as f64 / elapsed.as_secs_f64()) < LOW_SPEED_BYTES_PER_SEC as f64;
    *started_at = Instant::now();
    *bytes = 0;
    failed
}

fn transport_error(source_url: &str, error: reqwest::Error) -> DownloadError {
    let redirected = error
        .url()
        .is_some_and(|failed_url| !is_bmclapi_url(failed_url.as_str()));
    let message = if is_bmclapi_url(source_url) && redirected {
        format!(
            "{} -> BMCLAPI redirected node unavailable; retrying through the main endpoint ({})",
            source_url, error
        )
    } else {
        format!("{} -> {}", source_url, error)
    };
    DownloadError::new(ErrorKind::Transient, message)
}

pub(crate) async fn calculate_sha1(path: &Path) -> tokio::io::Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn job_needed(job: &DownloadJob) -> bool {
    let Ok(metadata) = fs::metadata(&job.dest).await else {
        return true;
    };

    if job.size.is_some_and(|expected| metadata.len() != expected) {
        return true;
    }

    if !job.check_hash {
        return false;
    }

    match job.sha1.as_deref() {
        Some(expected) => calculate_sha1(&job.dest)
            .await
            .map_or(true, |actual| actual != expected),
        None => false,
    }
}

fn file_check_concurrency(job_count: usize) -> usize {
    (job_count / 40).clamp(1, 8)
}

async fn monitor_speed(
    app_handle: tauri::AppHandle,
    downloaded_bytes: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    let started_at = Instant::now();
    let mut last_at = started_at;
    let mut last_bytes = 0usize;
    let mut speed_samples = VecDeque::<u64>::with_capacity(30);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    interval.tick().await;

    loop {
        interval.tick().await;
        let now = Instant::now();
        let total = downloaded_bytes.load(Ordering::SeqCst);
        let elapsed = now.duration_since(last_at).as_secs_f64();
        let actual = if elapsed > 0.0 {
            (total.saturating_sub(last_bytes) as f64 / elapsed) as u64
        } else {
            0
        };
        speed_samples.push_front(actual);
        if speed_samples.len() > 30 {
            speed_samples.pop_back();
        }
        let mut weighted_sum = 0u128;
        let mut weight_sum = 0u128;
        for (index, sample) in speed_samples.iter().enumerate() {
            let weight = (speed_samples.len() - index) as u128;
            weighted_sum += *sample as u128 * weight;
            weight_sum += weight;
        }
        let current = if weight_sum == 0 {
            0
        } else {
            (weighted_sum / weight_sum) as u64
        };
        let average = if started_at.elapsed().is_zero() {
            0
        } else {
            (total as f64 / started_at.elapsed().as_secs_f64()) as u64
        };
        last_at = now;
        last_bytes = total;

        let _ = app_handle.emit(
            "download-speed",
            DownloadSpeedPayload {
                average_bytes_per_sec: average,
                current_bytes_per_sec: current,
                low_speed: false,
                source: DownloadSource::R2,
            },
        );

        if stop.load(Ordering::SeqCst) {
            break;
        }
    }
}

pub(crate) async fn run_jobs(
    client: &Client,
    app_handle: &tauri::AppHandle,
    jobs: Vec<DownloadJob>,
    label: &str,
) -> Result<bool, String> {
    println!("[download] checking {} jobs: {}", label, jobs.len());
    let _ = app_handle.emit(
        "download-progress",
        ProgressPayload {
            current: 0,
            total: 0,
            file: format!("正在检查{}", label),
        },
    );

    let check_concurrency = file_check_concurrency(jobs.len());
    let checks = futures::stream::iter(jobs.into_iter().map(|job| {
        async move {
            let needed = job_needed(&job).await;
            (job, needed)
        }
        .boxed()
    }))
    .buffer_unordered(check_concurrency);

    let mut pending = Vec::new();
    let mut existing = 0usize;
    for (job, needed) in checks.collect::<Vec<_>>().await {
        if needed {
            println!(
                "[download] {} pending: {} <- {}",
                label,
                job.dest.display(),
                job.urls.join(" | ")
            );
            pending.push(job);
        } else {
            existing += 1;
        }
    }
    println!(
        "[download] {} check complete: {} existing, {} pending",
        label,
        existing,
        pending.len()
    );

    if pending.is_empty() {
        println!("[download] {} complete, no downloads needed", label);
        return Ok(false);
    }

    let total = pending.len();
    let completed = Arc::new(AtomicUsize::new(0));
    let downloaded_bytes = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let monitor = tokio::spawn(monitor_speed(
        app_handle.clone(),
        downloaded_bytes.clone(),
        stop.clone(),
    ));
    let file_semaphore = Arc::new(tokio::sync::Semaphore::new(FILE_CONCURRENCY));
    let client = client.clone();

    let transfers = futures::stream::iter(pending.into_iter().map(|job| {
        let client = client.clone();
        let app_handle = app_handle.clone();
        let completed = completed.clone();
        let downloaded_bytes = downloaded_bytes.clone();
        let file_semaphore = file_semaphore.clone();
        let label = label.to_string();

        async move {
            let _file_permit = file_semaphore
                .acquire()
                .await
                .map_err(|error| error.to_string())?;
            ensure_download(
                &client,
                &job.urls,
                &job.dest,
                job.sha1.as_deref(),
                job.size,
                Some(&downloaded_bytes),
            )
            .await?;
            let current = completed.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = app_handle.emit(
                "download-progress",
                ProgressPayload {
                    current,
                    total,
                    file: label,
                },
            );
            Ok::<(), String>(())
        }
        .boxed()
    }))
    .buffer_unordered(FILE_CONCURRENCY);

    let results = transfers.collect::<Vec<_>>().await;
    stop.store(true, Ordering::SeqCst);
    let _ = monitor.await;

    let errors = results
        .into_iter()
        .filter_map(Result::err)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        for error in &errors {
            eprintln!("[download] {} failed: {}", label, error);
        }
        return Err(format!(
            "{} 下载失败（{} 个文件）：{}",
            label,
            errors.len(),
            errors[0]
        ));
    }

    println!("[download] {} downloaded {} files", label, total);
    Ok(true)
}

pub(crate) async fn ensure_download(
    client: &Client,
    urls: &[String],
    dest: &Path,
    expected_hash: Option<&str>,
    expected_size: Option<u64>,
    downloaded_bytes: Option<&Arc<AtomicUsize>>,
) -> Result<(), String> {
    if let Ok(metadata) = fs::metadata(dest).await {
        if expected_size.is_some_and(|expected| metadata.len() != expected) {
            println!(
                "[download] local size mismatch, replacing {} (expected {}, got {})",
                dest.display(),
                expected_size.unwrap_or_default(),
                metadata.len()
            );
        } else if let Some(expected_hash) = expected_hash {
            if calculate_sha1(dest).await.ok().as_deref() == Some(expected_hash) {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }

    let mut last_error = None;
    for url in urls {
        for attempt in 0..MAX_ATTEMPTS {
            match download_file(
                client,
                url,
                dest,
                expected_hash,
                expected_size,
                downloaded_bytes,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let retryable = error.retryable();
                    let delay = retry_delay(error.kind, attempt);
                    last_error = Some(format!(
                        "{} (attempt {}/{})",
                        error,
                        attempt + 1,
                        MAX_ATTEMPTS
                    ));
                    if !retryable || attempt + 1 == MAX_ATTEMPTS {
                        break;
                    }
                    println!(
                        "[download] retry {}/{} in {:?}: {} ({})",
                        attempt + 1,
                        MAX_ATTEMPTS,
                        delay,
                        dest.display(),
                        error
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| format!("下载失败: {}", dest.display())))
}

async fn download_file(
    client: &Client,
    url: &str,
    dest: &Path,
    expected_hash: Option<&str>,
    expected_size: Option<u64>,
    downloaded_bytes: Option<&Arc<AtomicUsize>>,
) -> Result<(), DownloadError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
    }

    let tmp_path = temp_path(dest);
    download_single_stream(
        client,
        url,
        dest,
        &tmp_path,
        expected_hash,
        expected_size,
        downloaded_bytes,
    )
    .await
}

fn temp_path(dest: &Path) -> PathBuf {
    let mut path = dest.as_os_str().to_os_string();
    path.push(".download");
    PathBuf::from(path)
}

async fn download_single_stream(
    client: &Client,
    url: &str,
    dest: &Path,
    tmp_path: &Path,
    expected_hash: Option<&str>,
    expected_size: Option<u64>,
    downloaded_bytes: Option<&Arc<AtomicUsize>>,
) -> Result<(), DownloadError> {
    let _permit = acquire_request(url).await?;
    let mut resume_from = fs::metadata(tmp_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if expected_size.is_none() {
        let _ = fs::remove_file(tmp_path).await;
        resume_from = 0;
    }
    if expected_size.is_some_and(|size| resume_from >= size) {
        let _ = fs::remove_file(tmp_path).await;
        resume_from = 0;
    }

    let mut builder = request(client, url);
    if resume_from > 0 {
        builder = builder.header(RANGE, format!("bytes={}-", resume_from));
    }
    let response = builder
        .send()
        .await
        .map_err(|error| transport_error(url, error))?;
    if !response.status().is_success() {
        return Err(error_for_status(url, response.status()));
    }

    let can_resume = resume_from > 0
        && response.status() == StatusCode::PARTIAL_CONTENT
        && expected_size.is_some_and(|total| {
            content_range_matches(response.headers(), resume_from, total - 1, total)
        });
    if resume_from > 0 && !can_resume {
        let _ = fs::remove_file(tmp_path).await;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            return Err(DownloadError::new(
                ErrorKind::Transient,
                format!("{} -> invalid resume Content-Range", url),
            ));
        }
        resume_from = 0;
    }

    let mut hasher = Sha1::new();
    if resume_from > 0 {
        let mut existing = File::open(tmp_path)
            .await
            .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
        let mut buffer = vec![0u8; 128 * 1024];
        loop {
            let read = existing
                .read(&mut buffer)
                .await
                .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(resume_from == 0)
        .append(resume_from > 0)
        .open(tmp_path)
        .await
        .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
    let mut stream = response.bytes_stream();
    let mut written = resume_from;
    let mut speed_window_started = Instant::now();
    let mut speed_window_bytes = 0u64;

    loop {
        let item = tokio::time::timeout(NO_DATA_TIMEOUT, stream.next())
            .await
            .map_err(|_| {
                DownloadError::new(
                    ErrorKind::Transient,
                    format!("{} -> no data received for {:?}", url, NO_DATA_TIMEOUT),
                )
            })?;
        let Some(item) = item else {
            break;
        };
        let chunk =
            item.map_err(|error| DownloadError::new(ErrorKind::Transient, error.to_string()))?;
        written += chunk.len() as u64;
        speed_window_bytes += chunk.len() as u64;
        hasher.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
        if let Some(counter) = downloaded_bytes {
            counter.fetch_add(chunk.len(), Ordering::SeqCst);
        }
        if low_speed_window_failed(&mut speed_window_started, &mut speed_window_bytes) {
            return Err(DownloadError::new(
                ErrorKind::Transient,
                format!("{} -> transfer stayed below 1 KiB/s for 5 seconds", url),
            ));
        }
    }

    output
        .flush()
        .await
        .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
    drop(output);

    if expected_size.is_some_and(|size| size != written) {
        return Err(DownloadError::new(
            ErrorKind::Transient,
            format!(
                "{} -> size mismatch, expected {} bytes, got {} bytes",
                url,
                expected_size.unwrap_or_default(),
                written
            ),
        ));
    }

    let actual_hash = hex::encode(hasher.finalize());
    if expected_hash.is_some_and(|hash| actual_hash != hash) {
        let _ = fs::remove_file(tmp_path).await;
        return Err(DownloadError::new(
            ErrorKind::Integrity,
            format!("{} -> SHA-1 mismatch", url),
        ));
    }

    commit_file(tmp_path, dest).await
}

fn content_range_matches(
    headers: &reqwest::header::HeaderMap,
    start: u64,
    end: u64,
    total: u64,
) -> bool {
    headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == format!("bytes {}-{}/{}", start, end, total))
}

async fn commit_file(tmp_path: &Path, dest: &Path) -> Result<(), DownloadError> {
    if dest.exists() {
        fs::remove_file(dest)
            .await
            .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))?;
    }
    fs::rename(tmp_path, dest)
        .await
        .map_err(|error| DownloadError::new(ErrorKind::Permanent, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmclapi_request_pacing_matches_pcl() {
        assert_eq!(BMCLAPI_START_LANES, 2);
        assert_eq!(BMCLAPI_REQUEST_INTERVAL, Duration::from_millis(100));
    }

    #[test]
    fn retry_classification_matches_bmclapi_behavior() {
        assert_eq!(
            error_for_status("url", StatusCode::FORBIDDEN).kind,
            ErrorKind::RateLimited
        );
        assert_eq!(
            error_for_status("url", StatusCode::TOO_MANY_REQUESTS).kind,
            ErrorKind::RateLimited
        );
        assert_eq!(
            error_for_status("url", StatusCode::NOT_FOUND).kind,
            ErrorKind::Permanent
        );
        assert_eq!(
            error_for_status("url", StatusCode::SERVICE_UNAVAILABLE).kind,
            ErrorKind::Transient
        );
        assert_eq!(
            retry_delay(ErrorKind::Transient, 0),
            Duration::from_millis(250)
        );
        assert_eq!(
            retry_delay(ErrorKind::RateLimited, 0),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn download_futures_keep_large_buffers_off_the_stack() {
        let client = Client::new();
        let dest = PathBuf::from("unused-download-target");
        let download = download_file(
            &client,
            "https://example.invalid/file",
            &dest,
            None,
            Some(16 * 1024 * 1024),
            None,
        );
        let hash = calculate_sha1(&dest);

        assert!(std::mem::size_of_val(&download) < 64 * 1024);
        assert!(std::mem::size_of_val(&hash) < 16 * 1024);
    }

    #[tokio::test]
    async fn fast_existing_file_check_uses_size_without_hashing() {
        let path = std::env::temp_dir().join(format!(
            "circube-fast-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"abc").await.unwrap();

        let mut job = DownloadJob {
            urls: vec!["https://bmclapi2.bangbang93.com/assets/aa/hash".to_string()],
            dest: path.clone(),
            sha1: Some("intentionally-wrong".to_string()),
            size: Some(3),
            check_hash: false,
        };
        assert!(!job_needed(&job).await);

        job.check_hash = true;
        assert!(job_needed(&job).await);

        job.check_hash = false;
        job.size = Some(4);
        assert!(job_needed(&job).await);
        let _ = fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn single_stream_resumes_existing_temp_file() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 4096];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.contains("range: bytes=3-"), "{request}");
            socket
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef",
                )
                .await
                .unwrap();
        });

        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dest = std::env::temp_dir().join(format!("circube-resume-{suffix}"));
        let tmp = temp_path(&dest);
        fs::write(&tmp, b"abc").await.unwrap();

        download_single_stream(
            &Client::new(),
            &format!("http://{address}/file"),
            &dest,
            &tmp,
            None,
            Some(6),
            None,
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(fs::read(&dest).await.unwrap(), b"abcdef");
        assert!(!tmp.exists());
        let _ = fs::remove_file(dest).await;
    }
}
