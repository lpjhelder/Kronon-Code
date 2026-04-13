use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::terminal;

// Orange pulse tones for animation
const ORANGE_PULSE: [u8; 8] = [166, 172, 208, 214, 220, 214, 208, 172];

// --- Data types ---

#[derive(Debug, Clone)]
pub enum StatusPhase {
    Thinking,
    Streaming,
    ToolRunning(String),
    Done,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ToolActivity {
    pub name: String,
    pub summary: String,
    pub duration: Duration,
}

pub struct StatusBarState {
    pub phase: StatusPhase,
    pub started_at: Instant,
    /// Instant when the current phase started (resets on phase transitions so
    /// per-phase elapsed counters stay honest across iterations).
    pub phase_started_at: Instant,
    pub output_tokens: u32,
    pub estimated_tokens: u32,
    pub thinking_seconds: Option<f32>,
    pub tool_log: Vec<ToolActivity>,
    /// 1-based iteration count inside a multi-step turn (model → tool → model → ...).
    pub iteration: usize,
    /// Short name of the last tool that ran, so the "thinking" phase can say
    /// something meaningful like "processing grep_search result".
    pub last_tool: Option<String>,
    /// Resolved model id shown next to the elapsed counter.
    pub model: Option<String>,
    /// True when the status bar line has been printed and needs clearing before content
    pub line_visible: bool,
}

impl StatusBarState {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            phase: StatusPhase::Thinking,
            started_at: now,
            phase_started_at: now,
            output_tokens: 0,
            estimated_tokens: 0,
            thinking_seconds: None,
            tool_log: Vec::new(),
            iteration: 1,
            last_tool: None,
            model: None,
            line_visible: false,
        }
    }

    /// Atomically move to a new phase and reset the per-phase clock.
    pub fn enter_phase(&mut self, phase: StatusPhase) {
        self.phase = phase;
        self.phase_started_at = Instant::now();
    }
}

pub type StatusBarHandle = Arc<Mutex<StatusBarState>>;

// --- Rendering ---

fn thinking_verb(elapsed: u64, iteration: usize) -> &'static str {
    match (iteration, elapsed) {
        (1, 0..=9) => "Thinking...",
        (1, 10..=29) => "Still thinking...",
        (1, 30..=59) => "Waiting for model response...",
        (1, _) => "Model is taking a while — still waiting...",
        (_, 0..=9) => "Processing tool result...",
        (_, 10..=29) => "Still processing tool result...",
        (_, _) => "Waiting for model to continue...",
    }
}

fn render_status_line(state: &StatusBarState, frame: usize) -> String {
    match &state.phase {
        StatusPhase::Thinking => {
            // Elapsed inside the *current* phase, so a long tool run does not
            // inflate the "thinking" counter.
            let elapsed = state.phase_started_at.elapsed().as_secs();
            let tone = ORANGE_PULSE[frame % ORANGE_PULSE.len()];
            let verb = thinking_verb(elapsed, state.iteration);
            let model_suffix = state
                .model
                .as_deref()
                .map(|m| format!(" \u{00b7} {m}"))
                .unwrap_or_default();
            let iter_suffix = if state.iteration > 1 {
                format!(" \u{00b7} step {}", state.iteration)
            } else {
                String::new()
            };
            let tool_suffix = state
                .last_tool
                .as_deref()
                .filter(|_| state.iteration > 1)
                .map(|t| format!(" \u{00b7} after {t}"))
                .unwrap_or_default();
            format!(
                "\x1b[38;5;{tone}m\u{00b7} {verb} ({elapsed}s{iter_suffix}{tool_suffix}{model_suffix})\x1b[0m"
            )
        }
        StatusPhase::Streaming => {
            let elapsed = state.phase_started_at.elapsed().as_secs();
            let tokens = if state.output_tokens > 0 {
                state.output_tokens
            } else {
                state.estimated_tokens
            };
            let tone = ORANGE_PULSE[frame % ORANGE_PULSE.len()];
            let thinking = state
                .thinking_seconds
                .map(|s| format!(" \u{00b7} thought for {s:.0}s"))
                .unwrap_or_default();
            let iter_suffix = if state.iteration > 1 {
                format!(" \u{00b7} step {}", state.iteration)
            } else {
                String::new()
            };
            format!(
                "\x1b[38;5;{tone}m\u{00b7} Responding... ({elapsed}s \u{00b7} \u{2193} {tokens} tokens{iter_suffix}{thinking})\x1b[0m"
            )
        }
        StatusPhase::ToolRunning(name) => {
            let elapsed = state.phase_started_at.elapsed().as_secs();
            let iter_suffix = if state.iteration > 1 {
                format!(" \u{00b7} step {}", state.iteration)
            } else {
                String::new()
            };
            format!(
                "\x1b[32m\u{25cf}\x1b[0m \x1b[38;5;208m{name} ({elapsed}s{iter_suffix})\x1b[0m"
            )
        }
        StatusPhase::Done => {
            let elapsed = state.started_at.elapsed().as_secs();
            let tokens = if state.output_tokens > 0 {
                state.output_tokens
            } else {
                state.estimated_tokens
            };
            let tools = state.tool_log.len();
            format!(
                "\x1b[90m\u{00b7} Done ({elapsed}s \u{00b7} \u{2193} {tokens} tokens \u{00b7} {tools} tool calls)\x1b[0m"
            )
        }
        StatusPhase::Error(msg) => {
            format!("\x1b[31m\u{2718} {msg}\x1b[0m")
        }
    }
}

// --- StatusBarWriter ---

/// Writer that wraps stdout. Before each write, clears the status bar line
/// so content appears cleanly. The heartbeat thread repaints the status bar
/// in the gaps between writes.
pub struct StatusBarWriter {
    handle: StatusBarHandle,
    output_lock: Arc<Mutex<()>>,
}

impl StatusBarWriter {
    pub fn new(handle: StatusBarHandle, output_lock: Arc<Mutex<()>>) -> Self {
        Self {
            handle,
            output_lock,
        }
    }
}

impl Write for StatusBarWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _guard = self.output_lock.lock().unwrap();
        let mut stdout = io::stdout();

        // If status bar line is visible, clear it first
        {
            let mut state = self.handle.lock().unwrap();
            if state.line_visible {
                // Move to start of line and clear it
                write!(stdout, "\r\x1b[2K")?;
                state.line_visible = false;
            }
        }

        // Write content normally
        let written = stdout.write(buf)?;
        stdout.flush()?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

// --- StatusBar (facade) ---

pub struct StatusBar {
    handle: StatusBarHandle,
    output_lock: Arc<Mutex<()>>,
    pause_flag: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    heartbeat: Option<JoinHandle<()>>,
}

impl StatusBar {
    pub fn new() -> Self {
        let handle: StatusBarHandle = Arc::new(Mutex::new(StatusBarState::new()));
        let output_lock = Arc::new(Mutex::new(()));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let hb_handle = handle.clone();
        let hb_output_lock = output_lock.clone();
        let hb_pause = pause_flag.clone();
        let hb_stop = stop_flag.clone();

        let heartbeat = thread::spawn(move || {
            let mut frame = 0usize;
            loop {
                if hb_stop.load(Ordering::Relaxed) {
                    break;
                }
                if !hb_pause.load(Ordering::Relaxed) {
                    if let Ok(_guard) = hb_output_lock.try_lock() {
                        if let Ok(mut state) = hb_handle.lock() {
                            let line = render_status_line(&state, frame);
                            // Print status bar on a new line using \n, then use \r\x1b[2K
                            // to clear + \r\x1b[1A to move back up when content needs to write.
                            // Simpler: just print on current line with \r (overwrite in place)
                            let mut stdout = io::stdout();
                            if state.line_visible {
                                // Overwrite existing status bar in place
                                let _ = write!(stdout, "\r\x1b[2K{}", line);
                            } else {
                                // Print new status bar line
                                let _ = write!(stdout, "\n{}", line);
                                state.line_visible = true;
                            }
                            let _ = stdout.flush();
                        }
                    }
                }
                frame = frame.wrapping_add(1);
                thread::sleep(Duration::from_millis(50));
            }
        });

        Self {
            handle,
            output_lock,
            pause_flag,
            stop_flag,
            heartbeat: Some(heartbeat),
        }
    }

    pub fn handle(&self) -> StatusBarHandle {
        self.handle.clone()
    }

    pub fn output_lock(&self) -> Arc<Mutex<()>> {
        self.output_lock.clone()
    }

    pub fn pause_flag(&self) -> Arc<AtomicBool> {
        self.pause_flag.clone()
    }

    pub fn writer(&self) -> StatusBarWriter {
        StatusBarWriter::new(self.handle.clone(), self.output_lock.clone())
    }

    fn stop_heartbeat(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.heartbeat.take() {
            let _ = handle.join();
        }
    }

    pub fn freeze_done(&mut self) {
        self.stop_heartbeat();
        let _guard = self.output_lock.lock().unwrap();
        let mut state = self.handle.lock().unwrap();
        state.enter_phase(StatusPhase::Done);
        let line = render_status_line(&state, 0);
        let mut stdout = io::stdout();
        if state.line_visible {
            let _ = write!(stdout, "\r\x1b[2K{}", line);
        } else {
            let _ = write!(stdout, "\n{}", line);
        }
        state.line_visible = true;
        let _ = stdout.flush();
    }

    pub fn freeze_error(&mut self, msg: &str) {
        self.stop_heartbeat();
        let _guard = self.output_lock.lock().unwrap();
        let mut state = self.handle.lock().unwrap();
        state.enter_phase(StatusPhase::Error(msg.to_string()));
        let line = render_status_line(&state, 0);
        let mut stdout = io::stdout();
        if state.line_visible {
            let _ = write!(stdout, "\r\x1b[2K{}", line);
        } else {
            let _ = write!(stdout, "\n{}", line);
        }
        state.line_visible = true;
        let _ = stdout.flush();
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.heartbeat.take() {
            let _ = handle.join();
        }
    }
}

// Keep for external use but simplified
pub fn force_repaint(_handle: &StatusBarHandle, _output_lock: &Arc<Mutex<()>>, _frame: usize) {
    // No-op: heartbeat handles all repainting now
}
