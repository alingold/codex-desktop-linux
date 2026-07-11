//! Transactional Unicode text paste support for X11.
//!
//! X11 key injection is tied to the active keyboard layout, so synthesizing one
//! key event per character cannot reliably type arbitrary Unicode. This module
//! temporarily owns the `CLIPBOARD` selection with UTF-8 text, lets a caller
//! inject Ctrl+V through its preferred keyboard backend, and then restores the
//! previous plain-text selection.
//!
//! The implementation speaks the X11 selection protocol directly through the
//! pure-Rust `x11rb`/`x11-clipboard` crates. There is no runtime dependency on
//! `xclip`, `xsel`, a shell, or a clipboard manager. It requires a working X11
//! `DISPLAY` (and the usual Xauthority access), and is intentionally not a
//! Wayland clipboard fallback. Callers should use it only for an X11 session.
//!
//! Safety properties:
//!
//! - the previous selection is captured before input is emitted;
//! - rich/mixed-format selections, non-text selections, and snapshots above
//!   the configured bound are refused without changing the clipboard;
//! - the final owner comparison and temporary claim are protected by a short
//!   X server grab, closing the capture-to-claim race;
//! - selection reads have a deadline and support ICCCM `INCR` transfers;
//! - restoration is attempted on paste failure, timeout, panic, and future
//!   cancellation;
//! - restoration is skipped if another client took clipboard ownership in the
//!   meantime, so newer user data is never overwritten;
//! - failures before the paste callback are explicitly marked safe to fall back
//!   from, while failures after it begins must not be retried as text input.
//!
//! `X11Clipboard` keeps the restored text available by owning the X11 selection.
//! Store one long-lived instance in the Computer Use server instead of creating
//! an instance per call. A desktop clipboard manager may persist the selection
//! after process exit, but this module does not require or assume one. Because
//! the underlying owner can serve one payload target, mixed/rich clipboards are
//! deliberately left untouched and callers can choose another input backend.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use x11_clipboard::{Clipboard, Context as ClipboardContext};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, GetPropertyReply, Property, Window};
use x11rb::protocol::Event;
use x11rb::CURRENT_TIME;

const SNAPSHOT_ATTEMPTS: usize = 2;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Operational limits for an X11 clipboard paste transaction.
#[derive(Clone, Debug)]
pub struct X11ClipboardConfig {
    /// Maximum time to wait for the current selection owner to return each
    /// requested target.
    pub selection_timeout: Duration,
    /// Maximum time allowed for the asynchronous paste callback.
    pub paste_timeout: Duration,
    /// Smallest delay between Ctrl+V completion and clipboard restoration.
    pub restore_delay_min: Duration,
    /// Largest delay between Ctrl+V completion and clipboard restoration.
    pub restore_delay_max: Duration,
    /// Conservative transfer rate used to scale the restoration delay.
    pub assumed_transfer_bytes_per_second: u64,
    /// Maximum size of both text being pasted and the textual clipboard
    /// snapshot retained for restoration.
    pub max_clipboard_bytes: usize,
}

impl Default for X11ClipboardConfig {
    fn default() -> Self {
        Self {
            selection_timeout: Duration::from_secs(2),
            paste_timeout: Duration::from_secs(10),
            restore_delay_min: Duration::from_millis(1_500),
            restore_delay_max: Duration::from_secs(5),
            assumed_transfer_bytes_per_second: 256 * 1024,
            // The owner crate does not expose transfer-completion signals. Keep
            // the bounded heuristic conservative enough that a normal X11
            // paste consumer can finish before restoration.
            max_clipboard_bytes: 256 * 1024,
        }
    }
}

impl X11ClipboardConfig {
    fn validate(&self) -> Result<()> {
        if self.selection_timeout.is_zero() {
            bail!("X11 clipboard selection_timeout must be non-zero");
        }
        if self.paste_timeout.is_zero() {
            bail!("X11 clipboard paste_timeout must be non-zero");
        }
        if self.restore_delay_min > self.restore_delay_max {
            bail!("X11 clipboard restore_delay_min exceeds restore_delay_max");
        }
        if self.assumed_transfer_bytes_per_second == 0 {
            bail!("X11 clipboard assumed transfer rate must be non-zero");
        }
        if self.max_clipboard_bytes == 0 {
            bail!("X11 clipboard max_clipboard_bytes must be non-zero");
        }
        Ok(())
    }
}

/// Result of trying to put the user's previous clipboard state back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardRestoration {
    /// Previous text (including an empty text selection) is available again.
    RestoredText,
    /// The clipboard had no owner before the operation and is unowned again.
    RestoredUnowned,
    /// Another client changed the clipboard after the temporary text was set;
    /// its newer content was deliberately preserved.
    SkippedClipboardChanged {
        expected_owner: Window,
        current_owner: Option<Window>,
    },
    /// Restoration could not be confirmed. The paste itself may still have
    /// succeeded, so this is reported independently from paste success.
    Failed(String),
}

impl ClipboardRestoration {
    /// Whether the previous clipboard state was put back exactly as text or as
    /// an unowned selection.
    pub fn restored(&self) -> bool {
        matches!(self, Self::RestoredText | Self::RestoredUnowned)
    }
}

/// Successful clipboard-backed paste metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11ClipboardPasteReport {
    pub bytes: usize,
    pub restoration: ClipboardRestoration,
}

/// Whether a paste failure happened before or after input could have landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteErrorStage {
    BeforePaste,
    AfterPaste,
}

/// A paste error that tells callers whether another input backend is safe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11ClipboardPasteError {
    pub stage: PasteErrorStage,
    pub message: String,
    pub restoration: Option<ClipboardRestoration>,
}

impl X11ClipboardPasteError {
    fn before_paste(message: impl Into<String>) -> Self {
        Self {
            stage: PasteErrorStage::BeforePaste,
            message: message.into(),
            restoration: None,
        }
    }

    fn after_paste(message: impl Into<String>, restoration: ClipboardRestoration) -> Self {
        Self {
            stage: PasteErrorStage::AfterPaste,
            message: message.into(),
            restoration: Some(restoration),
        }
    }

    /// True only when the paste callback was never invoked, so another typing
    /// backend cannot duplicate partially delivered text.
    pub fn can_fallback_to_keyboard(&self) -> bool {
        self.stage == PasteErrorStage::BeforePaste
    }
}

impl fmt::Display for X11ClipboardPasteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)?;
        if let Some(restoration) = &self.restoration {
            write!(formatter, "; clipboard restoration: {restoration:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for X11ClipboardPasteError {}

/// A long-lived, transaction-serialized X11 clipboard backend.
#[derive(Clone)]
pub struct X11Clipboard {
    state: Arc<X11ClipboardState>,
}

struct X11ClipboardState {
    engine: Arc<dyn ClipboardEngine>,
    transaction: Arc<AsyncMutex<()>>,
    config: X11ClipboardConfig,
}

impl X11Clipboard {
    /// Connect to the X server selected by `DISPLAY` using production defaults.
    pub fn connect() -> Result<Self> {
        Self::connect_with_config(X11ClipboardConfig::default())
    }

    /// Connect with explicit limits. This does not modify or claim the
    /// clipboard until a paste transaction begins.
    pub fn connect_with_config(config: X11ClipboardConfig) -> Result<Self> {
        config.validate()?;
        let engine = NativeClipboardEngine::connect()
            .context("failed to initialize native X11 clipboard integration")?;
        Ok(Self::from_engine(Arc::new(engine), config))
    }

    fn from_engine(engine: Arc<dyn ClipboardEngine>, config: X11ClipboardConfig) -> Self {
        Self {
            state: Arc::new(X11ClipboardState {
                engine,
                transaction: Arc::new(AsyncMutex::new(())),
                config,
            }),
        }
    }

    /// Paste UTF-8 text with an async Ctrl+V callback.
    ///
    /// The callback should only inject the paste chord; focus and target
    /// verification should happen before calling this method. It is bounded by
    /// `paste_timeout`. If it returns an error or times out, restoration still
    /// runs and the returned error is marked `AfterPaste` so callers do not
    /// retry through another text backend.
    pub async fn paste_text<F, Fut, E>(
        &self,
        text: &str,
        paste: F,
    ) -> std::result::Result<X11ClipboardPasteReport, X11ClipboardPasteError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = std::result::Result<(), E>>,
        E: fmt::Display,
    {
        self.validate_text(text)?;
        let transaction = Arc::clone(&self.state.transaction).lock_owned().await;
        let restore_guard = self
            .prepare_async(text.as_bytes().to_vec(), transaction)
            .await?;

        let paste_result =
            match tokio::time::timeout(self.state.config.paste_timeout, paste()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(format!("X11 paste chord failed: {error}")),
                Err(_) => Err(format!(
                    "X11 paste chord timed out after {:?}",
                    self.state.config.paste_timeout
                )),
            };

        tokio::time::sleep(restore_delay(&self.state.config, text.len())).await;
        let restoration = restore_guard.restore_async().await;

        match paste_result {
            Ok(()) => Ok(X11ClipboardPasteReport {
                bytes: text.len(),
                restoration,
            }),
            Err(message) => Err(X11ClipboardPasteError::after_paste(message, restoration)),
        }
    }

    /// Synchronous counterpart for non-async callers.
    ///
    /// Clipboard reads are still selection-timeout bounded. The callback runs
    /// on the calling thread and therefore owns its own execution deadline;
    /// use `paste_text` when this module should enforce `paste_timeout` too.
    /// Do not call this method from inside an asynchronous Tokio task.
    pub fn paste_text_blocking<F, E>(
        &self,
        text: &str,
        paste: F,
    ) -> std::result::Result<X11ClipboardPasteReport, X11ClipboardPasteError>
    where
        F: FnOnce() -> std::result::Result<(), E>,
        E: fmt::Display,
    {
        self.validate_text(text)?;
        let _transaction = self.state.transaction.blocking_lock();
        let prepared = self
            .state
            .engine
            .prepare(text.as_bytes(), &self.state.config)
            .map_err(X11ClipboardPasteError::before_paste)?;
        let restore_guard = RestoreGuard::new(Arc::clone(&self.state.engine), prepared, None);

        let paste_result = paste().map_err(|error| format!("X11 paste chord failed: {error}"));
        thread::sleep(restore_delay(&self.state.config, text.len()));
        let restoration = restore_guard.restore_blocking();

        match paste_result {
            Ok(()) => Ok(X11ClipboardPasteReport {
                bytes: text.len(),
                restoration,
            }),
            Err(message) => Err(X11ClipboardPasteError::after_paste(message, restoration)),
        }
    }

    fn validate_text(&self, text: &str) -> std::result::Result<(), X11ClipboardPasteError> {
        if text.len() > self.state.config.max_clipboard_bytes {
            return Err(X11ClipboardPasteError::before_paste(format!(
                "X11 clipboard text is {} bytes; configured maximum is {} bytes",
                text.len(),
                self.state.config.max_clipboard_bytes
            )));
        }
        Ok(())
    }

    async fn prepare_async(
        &self,
        text: Vec<u8>,
        transaction: OwnedMutexGuard<()>,
    ) -> std::result::Result<RestoreGuard, X11ClipboardPasteError> {
        let engine = Arc::clone(&self.state.engine);
        let worker_engine = Arc::clone(&engine);
        let config = self.state.config.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();

        tokio::task::spawn_blocking(move || {
            let result = worker_engine.prepare(&text, &config);
            if let Err((Ok(prepared), _transaction)) = sender.send((result, transaction)) {
                // The caller was cancelled while the blocking selection read
                // was in progress. If preparation changed the clipboard, put
                // it back before `_transaction` releases the lock.
                let _ = worker_engine.restore(prepared);
            }
        });

        match receiver.await {
            Ok((Ok(prepared), transaction)) => {
                Ok(RestoreGuard::new(engine, prepared, Some(transaction)))
            }
            Ok((Err(error), _transaction)) => Err(X11ClipboardPasteError::before_paste(error)),
            Err(_) => Err(X11ClipboardPasteError::before_paste(
                "X11 clipboard preparation worker stopped unexpectedly",
            )),
        }
    }
}

fn restore_delay(config: &X11ClipboardConfig, bytes: usize) -> Duration {
    let transfer_ms = (bytes as u64)
        .saturating_mul(1_000)
        .div_ceil(config.assumed_transfer_bytes_per_second);
    Duration::from_millis(transfer_ms).clamp(config.restore_delay_min, config.restore_delay_max)
}

trait ClipboardEngine: Send + Sync {
    fn prepare(
        &self,
        text: &[u8],
        config: &X11ClipboardConfig,
    ) -> std::result::Result<PreparedClipboard, String>;

    fn restore(&self, prepared: PreparedClipboard) -> ClipboardRestoration;
}

struct RestoreGuard {
    engine: Arc<dyn ClipboardEngine>,
    prepared: Option<PreparedClipboard>,
    // When present, this guard deliberately outlives restoration. Its worker
    // handoff also keeps the lock held when the calling future is cancelled
    // during blocking X11 snapshot preparation.
    _transaction: Option<OwnedMutexGuard<()>>,
}

impl RestoreGuard {
    fn new(
        engine: Arc<dyn ClipboardEngine>,
        prepared: PreparedClipboard,
        transaction: Option<OwnedMutexGuard<()>>,
    ) -> Self {
        Self {
            engine,
            prepared: Some(prepared),
            _transaction: transaction,
        }
    }

    fn restore_blocking(mut self) -> ClipboardRestoration {
        let prepared = self
            .prepared
            .take()
            .expect("armed X11 clipboard restore guard");
        self.engine.restore(prepared)
    }

    async fn restore_async(mut self) -> ClipboardRestoration {
        let prepared = self
            .prepared
            .take()
            .expect("armed X11 clipboard restore guard");
        let engine = Arc::clone(&self.engine);
        let transaction = self._transaction.take();
        // Spawn before the first await. If this future is cancelled, the
        // blocking task still owns `prepared` and the transaction lock until
        // restoration completes.
        let worker = tokio::task::spawn_blocking(move || {
            let restoration = engine.restore(prepared);
            drop(transaction);
            restoration
        });
        match worker.await {
            Ok(restoration) => restoration,
            Err(error) => ClipboardRestoration::Failed(format!(
                "X11 clipboard restoration worker failed: {error}"
            )),
        }
    }
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if let Some(prepared) = self.prepared.take() {
            // This path covers cancellation and unwinding. It is intentionally
            // synchronous so the transaction lock cannot be released before
            // restoration has checked clipboard ownership.
            let _ = self.engine.restore(prepared);
        }
    }
}

#[derive(Clone, Debug)]
struct PreparedClipboard {
    snapshot: ClipboardSnapshot,
    temporary_owner: Window,
}

#[derive(Clone, Debug)]
struct CapturedClipboard {
    original_owner: Option<Window>,
    snapshot: ClipboardSnapshot,
}

#[derive(Clone, Debug)]
enum ClipboardSnapshot {
    Unowned,
    Text { target: Atom, bytes: Vec<u8> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailedClaimDisposition {
    RestoreSnapshot,
    ClipboardUnchanged,
    PreserveNewOwner,
}

fn failed_claim_disposition(
    original_owner: Option<Window>,
    temporary_owner: Window,
    current_owner: Option<Window>,
) -> FailedClaimDisposition {
    if current_owner == Some(temporary_owner) {
        FailedClaimDisposition::RestoreSnapshot
    } else if current_owner == original_owner {
        FailedClaimDisposition::ClipboardUnchanged
    } else {
        FailedClaimDisposition::PreserveNewOwner
    }
}

/// A very short X server grab closes the gap between validating the captured
/// owner and claiming the selection. The Drop path is a last-resort ungrab for
/// every early return and panic.
struct XServerGrab<'a> {
    context: &'a ClipboardContext,
    active: bool,
}

impl<'a> XServerGrab<'a> {
    fn acquire(context: &'a ClipboardContext) -> std::result::Result<Self, String> {
        context
            .connection
            .grab_server()
            .map_err(|error| format!("failed to request X server grab: {error}"))?
            .check()
            .map_err(|error| format!("failed to acquire X server grab: {error}"))?;
        Ok(Self {
            context,
            active: true,
        })
    }

    fn release(&mut self) -> std::result::Result<(), String> {
        self.context
            .connection
            .ungrab_server()
            .map_err(|error| format!("failed to request X server ungrab: {error}"))?
            .check()
            .map_err(|error| format!("failed to release X server grab: {error}"))?;
        self.context
            .connection
            .flush()
            .map_err(|error| format!("failed to flush X server ungrab: {error}"))?;
        self.active = false;
        Ok(())
    }
}

impl Drop for XServerGrab<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.context.connection.ungrab_server();
            let _ = self.context.connection.flush();
        }
    }
}

struct NativeClipboardEngine {
    clipboard: Mutex<Clipboard>,
}

impl NativeClipboardEngine {
    fn connect() -> Result<Self> {
        Ok(Self {
            clipboard: Mutex::new(Clipboard::new().context("X11 clipboard connection failed")?),
        })
    }
}

impl ClipboardEngine for NativeClipboardEngine {
    fn prepare(
        &self,
        text: &[u8],
        config: &X11ClipboardConfig,
    ) -> std::result::Result<PreparedClipboard, String> {
        let clipboard = self
            .clipboard
            .lock()
            .map_err(|_| "X11 clipboard lock is poisoned".to_string())?;
        let selection = clipboard.getter.atoms.clipboard;
        let captured = capture_snapshot(&clipboard, config)?;
        let temporary_owner = clipboard.setter.window;
        let mut server_grab = XServerGrab::acquire(&clipboard.setter)?;
        let owner_at_claim = selection_owner(&clipboard.setter, selection)?;
        if owner_at_claim != captured.original_owner {
            return Err(format!(
                "X11 clipboard owner changed after capture (was {:?}, now {owner_at_claim:?}); the newer clipboard was preserved",
                captured.original_owner
            ));
        }

        if let Err(error) =
            clipboard.store(selection, clipboard.setter.atoms.utf8_string, text.to_vec())
        {
            let cleanup = cleanup_failed_claim(
                &clipboard,
                &captured.snapshot,
                captured.original_owner,
                temporary_owner,
            );
            return Err(format!(
                "failed to set temporary X11 clipboard text: {error}; {cleanup}"
            ));
        }

        let owner_after_claim = match selection_owner(&clipboard.setter, selection) {
            Ok(owner) => owner,
            Err(error) => {
                let cleanup = cleanup_failed_claim(
                    &clipboard,
                    &captured.snapshot,
                    captured.original_owner,
                    temporary_owner,
                );
                return Err(format!(
                    "failed to verify temporary X11 clipboard ownership: {error}; {cleanup}"
                ));
            }
        };
        if owner_after_claim != Some(temporary_owner) {
            let cleanup = cleanup_failed_claim(
                &clipboard,
                &captured.snapshot,
                captured.original_owner,
                temporary_owner,
            );
            return Err(format!(
                "temporary X11 clipboard ownership was not acquired (owner {owner_after_claim:?}); {cleanup}"
            ));
        }

        if let Err(error) = server_grab.release() {
            let cleanup = cleanup_failed_claim(
                &clipboard,
                &captured.snapshot,
                captured.original_owner,
                temporary_owner,
            );
            return Err(format!(
                "temporary X11 clipboard claim could not be finalized: {error}; {cleanup}"
            ));
        }

        Ok(PreparedClipboard {
            snapshot: captured.snapshot,
            temporary_owner,
        })
    }

    fn restore(&self, prepared: PreparedClipboard) -> ClipboardRestoration {
        let clipboard = match self.clipboard.lock() {
            Ok(clipboard) => clipboard,
            Err(_) => {
                return ClipboardRestoration::Failed("X11 clipboard lock is poisoned".to_string())
            }
        };
        let selection = clipboard.getter.atoms.clipboard;
        let current_owner = match selection_owner(&clipboard.setter, selection) {
            Ok(owner) => owner,
            Err(error) => return ClipboardRestoration::Failed(error),
        };

        if current_owner != Some(prepared.temporary_owner) {
            return ClipboardRestoration::SkippedClipboardChanged {
                expected_owner: prepared.temporary_owner,
                current_owner,
            };
        }

        restore_snapshot(&clipboard, &prepared.snapshot)
    }
}

fn cleanup_failed_claim(
    clipboard: &Clipboard,
    snapshot: &ClipboardSnapshot,
    original_owner: Option<Window>,
    temporary_owner: Window,
) -> String {
    let selection = clipboard.getter.atoms.clipboard;
    let current_owner = match selection_owner(&clipboard.setter, selection) {
        Ok(owner) => owner,
        Err(error) => {
            return format!(
                "clipboard ownership could not be checked during cleanup ({error}); no destructive rollback was attempted"
            )
        }
    };
    match failed_claim_disposition(original_owner, temporary_owner, current_owner) {
        FailedClaimDisposition::RestoreSnapshot => {
            let restoration = restore_snapshot(clipboard, snapshot);
            format!("temporary ownership was rolled back: {restoration:?}")
        }
        FailedClaimDisposition::ClipboardUnchanged => {
            "the original clipboard owner remained in place".to_string()
        }
        FailedClaimDisposition::PreserveNewOwner => format!(
            "clipboard owner changed to {current_owner:?}; the newer clipboard was preserved"
        ),
    }
}

fn capture_snapshot(
    clipboard: &Clipboard,
    config: &X11ClipboardConfig,
) -> std::result::Result<CapturedClipboard, String> {
    let context = &clipboard.getter;
    let selection = context.atoms.clipboard;

    for attempt in 0..SNAPSHOT_ATTEMPTS {
        let owner_before = selection_owner(context, selection)?;
        if owner_before.is_none() {
            if selection_owner(context, selection)?.is_none() {
                return Ok(CapturedClipboard {
                    original_owner: None,
                    snapshot: ClipboardSnapshot::Unowned,
                });
            }
            continue;
        }

        let targets = request_selection(
            context,
            selection,
            context.atoms.targets,
            AtomEnum::ATOM.into(),
            32,
            config.selection_timeout,
            config.max_clipboard_bytes,
        )?
        .ok_or_else(|| "current X11 clipboard owner does not offer TARGETS".to_string())?;
        let advertised_targets = atoms_from_property(&targets)?;
        let target = select_plain_text_target(context, &advertised_targets)?;

        let text = request_selection(
            context,
            selection,
            target,
            target,
            8,
            config.selection_timeout,
            config.max_clipboard_bytes,
        )?
        .ok_or_else(|| {
            "current X11 clipboard owner refused its advertised text target".to_string()
        })?;

        if selection_owner(context, selection)? == owner_before {
            return Ok(CapturedClipboard {
                original_owner: owner_before,
                snapshot: ClipboardSnapshot::Text {
                    target,
                    bytes: text.value,
                },
            });
        }

        if attempt + 1 == SNAPSHOT_ATTEMPTS {
            return Err("X11 clipboard changed repeatedly while it was being captured".to_string());
        }
    }

    Err("X11 clipboard could not be captured consistently".to_string())
}

fn select_plain_text_target(
    context: &ClipboardContext,
    targets: &[Atom],
) -> std::result::Result<Atom, String> {
    let mut preferred = vec![context.atoms.utf8_string];
    preferred.extend(intern_atoms(
        context,
        &[
            "text/plain;charset=utf-8",
            "text/plain;charset=UTF-8",
            "text/plain",
            "COMPOUND_TEXT",
            "TEXT",
        ],
    )?);
    preferred.push(context.atoms.string);

    // These protocol/metadata targets do not carry richer clipboard content.
    // Losing an auxiliary query after restoration does not change the semantic
    // plain-text payload; any unknown payload target is rejected conservatively.
    let mut allowed = preferred.clone();
    allowed.push(context.atoms.targets);
    allowed.extend(intern_atoms(
        context,
        &[
            "TIMESTAMP",
            "MULTIPLE",
            "SAVE_TARGETS",
            "DELETE",
            "INSERT_SELECTION",
            "INSERT_PROPERTY",
            "LENGTH",
            "LIST_LENGTH",
            "CHARACTER_POSITION",
            "NAME",
            "CLASS",
            "CLIENT_WINDOW",
            "OWNER_OS",
            "HOST_NAME",
            "USER",
            "SPAN",
        ],
    )?);

    let unexpected = unsupported_clipboard_targets(targets, &allowed);
    if !unexpected.is_empty() {
        let labels = unexpected
            .into_iter()
            .map(|atom| atom_label(context, atom))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "current X11 clipboard offers rich or mixed-format target(s) ({labels}); it was left untouched because this backend can restore only semantic plain text"
        ));
    }

    preferred
        .into_iter()
        .find(|target| targets.contains(target))
        .ok_or_else(|| {
            "current X11 clipboard is not plain text (no supported text target is available)"
                .to_string()
        })
}

fn unsupported_clipboard_targets(targets: &[Atom], allowed: &[Atom]) -> Vec<Atom> {
    targets
        .iter()
        .copied()
        .filter(|target| !allowed.contains(target))
        .collect()
}

fn intern_atoms(
    context: &ClipboardContext,
    names: &[&str],
) -> std::result::Result<Vec<Atom>, String> {
    names
        .iter()
        .map(|name| {
            context
                .get_atom(name)
                .map_err(|error| format!("failed to intern X11 clipboard atom {name}: {error}"))
        })
        .collect()
}

fn atom_label(context: &ClipboardContext, atom: Atom) -> String {
    context
        .connection
        .get_atom_name(atom)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| atom.to_string())
}

fn atoms_from_property(property: &SelectionProperty) -> std::result::Result<Vec<Atom>, String> {
    if property.format != 32 || !property.value.len().is_multiple_of(4) {
        return Err("X11 TARGETS response is not a 32-bit atom array".to_string());
    }
    Ok(property
        .value
        .chunks_exact(4)
        .map(|bytes| Atom::from_ne_bytes(bytes.try_into().expect("four-byte atom")))
        .collect())
}

fn restore_snapshot(clipboard: &Clipboard, snapshot: &ClipboardSnapshot) -> ClipboardRestoration {
    let selection = clipboard.getter.atoms.clipboard;
    match snapshot {
        ClipboardSnapshot::Unowned => {
            let no_owner: Window = AtomEnum::NONE.into();
            let result = clipboard
                .setter
                .connection
                .set_selection_owner(no_owner, selection, CURRENT_TIME)
                .map_err(|error| format!("failed to clear X11 clipboard ownership: {error}"))
                .and_then(|cookie| {
                    cookie.check().map_err(|error| {
                        format!("failed to clear X11 clipboard ownership: {error}")
                    })
                });
            match result.and_then(|()| selection_owner(&clipboard.setter, selection)) {
                Ok(None) => ClipboardRestoration::RestoredUnowned,
                Ok(owner) => ClipboardRestoration::Failed(format!(
                    "X11 clipboard should be unowned after restoration, but owner is {owner:?}"
                )),
                Err(error) => ClipboardRestoration::Failed(error),
            }
        }
        ClipboardSnapshot::Text { target, bytes } => {
            if let Err(error) = clipboard.store(selection, *target, bytes.clone()) {
                return ClipboardRestoration::Failed(format!(
                    "failed to restore X11 clipboard text: {error}"
                ));
            }
            match selection_owner(&clipboard.setter, selection) {
                Ok(owner) if owner == Some(clipboard.setter.window) => {
                    ClipboardRestoration::RestoredText
                }
                Ok(owner) => ClipboardRestoration::Failed(format!(
                    "restored X11 clipboard ownership was not retained (owner {owner:?})"
                )),
                Err(error) => ClipboardRestoration::Failed(error),
            }
        }
    }
}

fn selection_owner(
    context: &ClipboardContext,
    selection: Atom,
) -> std::result::Result<Option<Window>, String> {
    let owner = context
        .connection
        .get_selection_owner(selection)
        .map_err(|error| format!("failed to request X11 clipboard owner: {error}"))?
        .reply()
        .map_err(|error| format!("failed to read X11 clipboard owner: {error}"))?
        .owner;
    let no_owner: Window = AtomEnum::NONE.into();
    Ok((owner != no_owner).then_some(owner))
}

#[derive(Debug)]
struct SelectionProperty {
    format: u8,
    value: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn request_selection(
    context: &ClipboardContext,
    selection: Atom,
    target: Atom,
    expected_type: Atom,
    expected_format: u8,
    timeout: Duration,
    max_bytes: usize,
) -> std::result::Result<Option<SelectionProperty>, String> {
    let connection = &context.connection;
    let property = context.atoms.property;
    connection
        .delete_property(context.window, property)
        .map_err(|error| format!("failed to clear X11 clipboard request property: {error}"))?
        .check()
        .map_err(|error| format!("failed to clear X11 clipboard request property: {error}"))?;
    connection
        .convert_selection(context.window, selection, target, property, CURRENT_TIME)
        .map_err(|error| format!("failed to request X11 clipboard target: {error}"))?
        .check()
        .map_err(|error| format!("failed to request X11 clipboard target: {error}"))?;
    connection
        .flush()
        .map_err(|error| format!("failed to flush X11 clipboard request: {error}"))?;

    let deadline = Instant::now() + timeout;
    let mut incremental = false;
    let mut value = Vec::new();

    loop {
        if Instant::now() >= deadline {
            let _ = connection.delete_property(context.window, property);
            let _ = connection.flush();
            return Err(format!(
                "X11 clipboard target request timed out after {timeout:?}"
            ));
        }

        let event = connection
            .poll_for_event()
            .map_err(|error| format!("failed to poll X11 clipboard events: {error}"))?;
        let Some(event) = event else {
            thread::sleep(
                EVENT_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            );
            continue;
        };

        match event {
            Event::SelectionNotify(event)
                if event.requestor == context.window
                    && event.selection == selection
                    && event.target == target =>
            {
                let no_property: Atom = AtomEnum::NONE.into();
                if event.property == no_property {
                    return Ok(None);
                }
                let reply = read_property(context, max_bytes)?;
                if reply.type_ == context.atoms.incr {
                    if let Some(size) = reply.value32().and_then(|mut values| values.next()) {
                        if size as usize > max_bytes {
                            return Err(format!(
                                "X11 clipboard snapshot is at least {size} bytes; configured maximum is {max_bytes} bytes"
                            ));
                        }
                    }
                    connection
                        .delete_property(context.window, property)
                        .map_err(|error| format!("failed to begin X11 INCR transfer: {error}"))?
                        .check()
                        .map_err(|error| format!("failed to begin X11 INCR transfer: {error}"))?;
                    connection
                        .flush()
                        .map_err(|error| format!("failed to flush X11 INCR transfer: {error}"))?;
                    incremental = true;
                } else {
                    validate_property(&reply, expected_type, expected_format, max_bytes)?;
                    return Ok(Some(SelectionProperty {
                        format: reply.format,
                        value: reply.value,
                    }));
                }
            }
            Event::PropertyNotify(event)
                if incremental
                    && event.window == context.window
                    && event.atom == property
                    && event.state == Property::NEW_VALUE =>
            {
                let reply = read_property(context, max_bytes.saturating_sub(value.len()))?;
                if reply.value.is_empty() {
                    return Ok(Some(SelectionProperty {
                        format: expected_format,
                        value,
                    }));
                }
                validate_property(
                    &reply,
                    expected_type,
                    expected_format,
                    max_bytes.saturating_sub(value.len()),
                )?;
                if value.len().saturating_add(reply.value.len()) > max_bytes {
                    return Err(format!(
                        "X11 clipboard snapshot exceeds configured maximum of {max_bytes} bytes"
                    ));
                }
                value.extend_from_slice(&reply.value);
            }
            _ => {}
        }
    }
}

fn read_property(
    context: &ClipboardContext,
    remaining_bytes: usize,
) -> std::result::Result<GetPropertyReply, String> {
    let long_length = remaining_bytes
        .div_ceil(4)
        .saturating_add(1)
        .min(u32::MAX as usize) as u32;
    context
        .connection
        .get_property(
            true,
            context.window,
            context.atoms.property,
            AtomEnum::ANY,
            0,
            long_length,
        )
        .map_err(|error| format!("failed to request X11 clipboard property: {error}"))?
        .reply()
        .map_err(|error| format!("failed to read X11 clipboard property: {error}"))
}

fn validate_property(
    reply: &GetPropertyReply,
    expected_type: Atom,
    expected_format: u8,
    max_bytes: usize,
) -> std::result::Result<(), String> {
    if reply.type_ != expected_type {
        return Err(format!(
            "X11 clipboard returned unexpected property type {} (expected {expected_type})",
            reply.type_
        ));
    }
    if reply.format != expected_format {
        return Err(format!(
            "X11 clipboard returned {}-bit data (expected {expected_format}-bit)",
            reply.format
        ));
    }
    if reply.value.len() > max_bytes || reply.bytes_after > 0 {
        return Err(format!(
            "X11 clipboard snapshot exceeds configured maximum of {max_bytes} bytes"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FakeEngine {
        prepare_error: Option<String>,
        restoration: ClipboardRestoration,
        prepare_calls: AtomicUsize,
        restore_calls: AtomicUsize,
        pasted_bytes: Mutex<Vec<u8>>,
        prepare_delay: Duration,
        restore_delay: Duration,
    }

    impl FakeEngine {
        fn successful(restoration: ClipboardRestoration) -> Self {
            Self {
                prepare_error: None,
                restoration,
                prepare_calls: AtomicUsize::new(0),
                restore_calls: AtomicUsize::new(0),
                pasted_bytes: Mutex::new(Vec::new()),
                prepare_delay: Duration::ZERO,
                restore_delay: Duration::ZERO,
            }
        }
    }

    impl ClipboardEngine for FakeEngine {
        fn prepare(
            &self,
            text: &[u8],
            _config: &X11ClipboardConfig,
        ) -> std::result::Result<PreparedClipboard, String> {
            self.prepare_calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(self.prepare_delay);
            if let Some(error) = &self.prepare_error {
                return Err(error.clone());
            }
            *self.pasted_bytes.lock().expect("fake bytes lock") = text.to_vec();
            Ok(PreparedClipboard {
                snapshot: ClipboardSnapshot::Unowned,
                temporary_owner: 42,
            })
        }

        fn restore(&self, _prepared: PreparedClipboard) -> ClipboardRestoration {
            self.restore_calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(self.restore_delay);
            self.restoration.clone()
        }
    }

    fn test_config() -> X11ClipboardConfig {
        X11ClipboardConfig {
            selection_timeout: Duration::from_millis(50),
            paste_timeout: Duration::from_millis(50),
            restore_delay_min: Duration::ZERO,
            restore_delay_max: Duration::ZERO,
            assumed_transfer_bytes_per_second: 1_000,
            max_clipboard_bytes: 128,
        }
    }

    #[test]
    fn default_config_is_valid() {
        X11ClipboardConfig::default().validate().unwrap();
    }

    #[test]
    fn config_rejects_inverted_restore_delay() {
        let mut config = test_config();
        config.restore_delay_min = Duration::from_secs(2);
        config.restore_delay_max = Duration::from_secs(1);
        assert!(config.validate().is_err());
    }

    #[test]
    fn restore_delay_scales_and_clamps() {
        let config = X11ClipboardConfig {
            restore_delay_min: Duration::from_millis(10),
            restore_delay_max: Duration::from_millis(100),
            assumed_transfer_bytes_per_second: 1_000,
            ..test_config()
        };
        assert_eq!(restore_delay(&config, 1), Duration::from_millis(10));
        assert_eq!(restore_delay(&config, 50), Duration::from_millis(50));
        assert_eq!(restore_delay(&config, 1_000), Duration::from_millis(100));
    }

    #[test]
    fn restoration_reports_exact_success_only() {
        assert!(ClipboardRestoration::RestoredText.restored());
        assert!(ClipboardRestoration::RestoredUnowned.restored());
        assert!(!ClipboardRestoration::SkippedClipboardChanged {
            expected_owner: 1,
            current_owner: Some(2),
        }
        .restored());
        assert!(!ClipboardRestoration::Failed("no".to_string()).restored());
    }

    #[test]
    fn failed_claim_cleanup_never_overwrites_a_new_owner() {
        let original = Some(11);
        let temporary = 22;

        assert_eq!(
            failed_claim_disposition(original, temporary, Some(temporary)),
            FailedClaimDisposition::RestoreSnapshot
        );
        assert_eq!(
            failed_claim_disposition(original, temporary, original),
            FailedClaimDisposition::ClipboardUnchanged
        );
        assert_eq!(
            failed_claim_disposition(original, temporary, Some(33)),
            FailedClaimDisposition::PreserveNewOwner
        );
        assert_eq!(
            failed_claim_disposition(None, temporary, Some(33)),
            FailedClaimDisposition::PreserveNewOwner
        );
        assert_eq!(
            failed_claim_disposition(None, temporary, None),
            FailedClaimDisposition::ClipboardUnchanged
        );
    }

    #[test]
    fn unknown_rich_targets_are_rejected_by_plain_text_filter() {
        let targets = [1, 2, 3, 4];
        let allowed = [1, 2, 4];
        assert_eq!(unsupported_clipboard_targets(&targets, &allowed), vec![3]);
    }

    #[tokio::test]
    async fn async_paste_preserves_unicode_and_restores() {
        let engine = Arc::new(FakeEngine::successful(ClipboardRestoration::RestoredText));
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());
        let callback_ran = Arc::new(AtomicBool::new(false));
        let callback_state = Arc::clone(&callback_ran);

        let report = clipboard
            .paste_text("Grüße 🌍 — こんにちは", move || async move {
                callback_state.store(true, Ordering::SeqCst);
                Ok::<_, String>(())
            })
            .await
            .unwrap();

        assert!(callback_ran.load(Ordering::SeqCst));
        assert_eq!(report.bytes, "Grüße 🌍 — こんにちは".len());
        assert_eq!(report.restoration, ClipboardRestoration::RestoredText);
        assert_eq!(engine.prepare_calls.load(Ordering::SeqCst), 1);
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *engine.pasted_bytes.lock().unwrap(),
            "Grüße 🌍 — こんにちは".as_bytes()
        );
    }

    #[tokio::test]
    async fn preparation_failure_is_safe_to_fallback_and_does_not_paste() {
        let engine = Arc::new(FakeEngine {
            prepare_error: Some("clipboard is image-only".to_string()),
            ..FakeEngine::successful(ClipboardRestoration::RestoredText)
        });
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());
        let callback_ran = Arc::new(AtomicBool::new(false));
        let callback_state = Arc::clone(&callback_ran);

        let error = clipboard
            .paste_text("hello", move || async move {
                callback_state.store(true, Ordering::SeqCst);
                Ok::<_, String>(())
            })
            .await
            .unwrap_err();

        assert!(error.can_fallback_to_keyboard());
        assert_eq!(error.stage, PasteErrorStage::BeforePaste);
        assert!(!callback_ran.load(Ordering::SeqCst));
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn paste_failure_restores_and_must_not_fallback() {
        let engine = Arc::new(FakeEngine::successful(
            ClipboardRestoration::RestoredUnowned,
        ));
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());

        let error = clipboard
            .paste_text("hello", || async { Err::<(), _>("input denied") })
            .await
            .unwrap_err();

        assert!(!error.can_fallback_to_keyboard());
        assert_eq!(error.stage, PasteErrorStage::AfterPaste);
        assert_eq!(
            error.restoration,
            Some(ClipboardRestoration::RestoredUnowned)
        );
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn paste_timeout_restores_and_must_not_fallback() {
        let engine = Arc::new(FakeEngine::successful(ClipboardRestoration::RestoredText));
        let mut config = test_config();
        config.paste_timeout = Duration::from_millis(5);
        let clipboard = X11Clipboard::from_engine(engine.clone(), config);

        let error = clipboard
            .paste_text("hello", || async {
                std::future::pending::<std::result::Result<(), String>>().await
            })
            .await
            .unwrap_err();

        assert_eq!(error.stage, PasteErrorStage::AfterPaste);
        assert!(error.message.contains("timed out"));
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_during_prepare_holds_lock_until_restoration() {
        let engine = Arc::new(FakeEngine {
            prepare_delay: Duration::from_millis(40),
            ..FakeEngine::successful(ClipboardRestoration::RestoredText)
        });
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());
        let first_clipboard = clipboard.clone();
        let first = tokio::spawn(async move {
            first_clipboard
                .paste_text("first", || async { Ok::<_, String>(()) })
                .await
        });

        while engine.prepare_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        first.abort();

        let second_clipboard = clipboard.clone();
        let second = tokio::spawn(async move {
            second_clipboard
                .paste_text("second", || async { Ok::<_, String>(()) })
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            engine.prepare_calls.load(Ordering::SeqCst),
            1,
            "a cancelled snapshot worker must retain transaction ownership"
        );

        second.await.unwrap().unwrap();
        assert_eq!(engine.prepare_calls.load(Ordering::SeqCst), 2);
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancellation_during_restore_holds_lock_until_worker_finishes() {
        let engine = Arc::new(FakeEngine {
            restore_delay: Duration::from_millis(40),
            ..FakeEngine::successful(ClipboardRestoration::RestoredText)
        });
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());
        let first_clipboard = clipboard.clone();
        let first = tokio::spawn(async move {
            first_clipboard
                .paste_text("first", || async { Ok::<_, String>(()) })
                .await
        });

        while engine.restore_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        first.abort();

        let second_clipboard = clipboard.clone();
        let second = tokio::spawn(async move {
            second_clipboard
                .paste_text("second", || async { Ok::<_, String>(()) })
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            engine.prepare_calls.load(Ordering::SeqCst),
            1,
            "a cancelled restore future must leave its worker holding the transaction lock"
        );

        second.await.unwrap().unwrap();
        assert_eq!(engine.prepare_calls.load(Ordering::SeqCst), 2);
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn oversized_text_is_rejected_before_clipboard_mutation() {
        let engine = Arc::new(FakeEngine::successful(ClipboardRestoration::RestoredText));
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());

        let error = clipboard
            .paste_text(&"x".repeat(129), || async { Ok::<_, String>(()) })
            .await
            .unwrap_err();

        assert!(error.can_fallback_to_keyboard());
        assert_eq!(engine.prepare_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn blocking_paste_uses_same_transaction_contract() {
        let engine = Arc::new(FakeEngine::successful(
            ClipboardRestoration::SkippedClipboardChanged {
                expected_owner: 42,
                current_owner: Some(99),
            },
        ));
        let clipboard = X11Clipboard::from_engine(engine.clone(), test_config());

        let report = clipboard
            .paste_text_blocking("literal", || Ok::<_, String>(()))
            .unwrap();

        assert_eq!(
            report.restoration,
            ClipboardRestoration::SkippedClipboardChanged {
                expected_owner: 42,
                current_owner: Some(99),
            }
        );
        assert_eq!(engine.restore_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn atom_array_parsing_rejects_misaligned_values() {
        let property = SelectionProperty {
            format: 32,
            value: vec![1, 2, 3],
        };
        assert!(atoms_from_property(&property).is_err());
    }
}
