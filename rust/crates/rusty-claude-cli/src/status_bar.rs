use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::cursor::{MoveTo, RestorePosition, SavePosition};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, style::Print};

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
    pub output_tokens: u32,
    pub thinking_seconds: Option<f32>,
    pub tool_log: Vec<ToolActivity>,
}

impl StatusBarState {
    fn new() -> Self {
        Self {
            phase: StatusPhase::Thinking,
            started_at: Instant::now(),
            output_tokens: 0,
            thinking_seconds: None,
            tool_log: Vec::new(),
        }
    }
}

pub type StatusBarHandle = Arc<Mutex<StatusBarState>>;

// --- Rendering ---

fn render_status_line(state: &StatusBarState, frame: usize, width: u16) -> String {
    let raw = match &state.phase {
        StatusPhase::Thinking => {
            let elapsed = state.started_at.elapsed().as_secs();
            let tone = ORANGE_PULSE[frame % ORANGE_PULSE.len()];
            format!("\x1b[38;5;{tone}m\u{00b7} Thinking... ({elapsed}s)\x1b[0m")
        }
        StatusPhase::Streaming => {
            let elapsed = state.started_at.elapsed().as_secs();
            let tokens = state.output_tokens;
            let tone = ORANGE_PULSE[frame % ORANGE_PULSE.len()];
            let thinking = state
                .thinking_seconds
                .map(|s| format!(" \u{00b7} thought for {s:.0}s"))
                .unwrap_or_default();
            format!(
                "\x1b[38;5;{tone}m\u{00b7} Transfiguring... ({elapsed}s \u{00b7} \u{2193} {tokens} tokens{thinking})\x1b[0m"
            )
        }
        StatusPhase::ToolRunning(name) => {
            let elapsed = state.started_at.elapsed().as_secs();
            format!(
                "\x1b[32m\u{25cf}\x1b[0m \x1b[38;5;208m{name} ({elapsed}s)\x1b[0m"
            )
        }
        StatusPhase::Done => {
            let elapsed = state.started_at.elapsed().as_secs();
            let tokens = state.output_tokens;
            let tools = state.tool_log.len();
            format!(
                "\x1b[90m\u{00b7} Done ({elapsed}s \u{00b7} \u{2193} {tokens} tokens \u{00b7} {tools} tool calls)\x1b[0m"
            )
        }
        StatusPhase::Error(msg) => {
            format!("\x1b[31m\u{2718} {msg}\x1b[0m")
        }
    };
    // Truncate to terminal width (accounting for ANSI escape codes)
    // Simple approach: just cap the visible string. ANSI codes are invisible.
    let _ = width; // Width used for future truncation logic
    raw
}

// --- StatusBarWriter ---

pub struct StatusBarWriter {
    handle: StatusBarHandle,
    output_lock: Arc<Mutex<()>>,
    frame: usize,
}

impl StatusBarWriter {
    pub fn new(handle: StatusBarHandle, output_lock: Arc<Mutex<()>>) -> Self {
        Self {
            handle,
            output_lock,
            frame: 0,
        }
    }

    fn repaint_status_bar(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let state = self.handle.lock().unwrap();
        let line = render_status_line(&state, self.frame, width);
        execute!(
            stdout,
            SavePosition,
            MoveTo(0, height - 1),
            Clear(ClearType::CurrentLine),
            Print(line),
            RestorePosition
        )?;
        Ok(())
    }
}

impl Write for StatusBarWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _guard = self.output_lock.lock().unwrap();
        let mut stdout = io::stdout();

        // 1. Clear the status bar line so content can use the full terminal
        let (_, height) = terminal::size().unwrap_or((80, 24));
        execute!(
            stdout,
            SavePosition,
            MoveTo(0, height - 1),
            Clear(ClearType::CurrentLine),
            RestorePosition
        )?;

        // 2. Write the actual content (may cause scrolling)
        let written = stdout.write(buf)?;
        stdout.flush()?;

        // 3. Repaint the status bar at the bottom
        self.frame = self.frame.wrapping_add(1);
        self.repaint_status_bar(&mut stdout)?;

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
                        let state = hb_handle.lock().unwrap();
                        let (width, height) = terminal::size().unwrap_or((80, 24));
                        let line = render_status_line(&state, frame, width);
                        drop(state); // Release state lock before writing
                        let mut stdout = io::stdout();
                        let _ = execute!(
                            stdout,
                            SavePosition,
                            MoveTo(0, height - 1),
                            Clear(ClearType::CurrentLine),
                            Print(line),
                            RestorePosition
                        );
                        let _ = stdout.flush();
                    }
                    // If lock is held by writer, skip this tick
                }
                frame = frame.wrapping_add(1);
                thread::sleep(Duration::from_millis(80));
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
        {
            let mut state = self.handle.lock().unwrap();
            state.phase = StatusPhase::Done;
        }
        self.repaint_final();
    }

    pub fn freeze_error(&mut self, msg: &str) {
        self.stop_heartbeat();
        {
            let mut state = self.handle.lock().unwrap();
            state.phase = StatusPhase::Error(msg.to_string());
        }
        self.repaint_final();
    }

    fn repaint_final(&self) {
        let _guard = self.output_lock.lock().unwrap();
        let state = self.handle.lock().unwrap();
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let line = render_status_line(&state, 0, width);
        drop(state);
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            SavePosition,
            MoveTo(0, height - 1),
            Clear(ClearType::CurrentLine),
            Print(line),
            RestorePosition
        );
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
