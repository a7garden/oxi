use std::io::{Read, Write, stdout, stdin};
use std::thread;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use crossterm::{execute, cursor};
use crossterm::terminal::{self, ClearType};

/// Terminal trait for abstracting terminal I/O operations
pub trait Terminal: Send + Sync {
    /// Start the terminal with callbacks for input and resize events
    fn start(&mut self, on_input: Box<dyn Fn(String) + Send>, on_resize: Box<dyn Fn() + Send>) -> Result<()>;
    /// Stop the terminal
    fn stop(&mut self);
    /// Write data to the terminal
    fn write(&self, data: &str);
    /// Get number of columns
    fn columns(&self) -> usize;
    /// Get number of rows
    fn rows(&self) -> usize;
    /// Hide the cursor
    fn hide_cursor(&self);
    /// Show the cursor
    fn show_cursor(&self);
    /// Clear the current line
    fn clear_line(&self);
    /// Clear the entire screen
    fn clear_screen(&self);
}

/// Process-based terminal implementation
pub struct ProcessTerminal {
    running: Arc<AtomicBool>,
    columns: Arc<AtomicUsize>,
    rows: Arc<AtomicUsize>,
}

impl ProcessTerminal {
    pub fn new() -> Self {
        let (cols, rows) = Self::detect_size();
        Self {
            running: Arc::new(AtomicBool::new(false)),
            columns: Arc::new(AtomicUsize::new(cols)),
            rows: Arc::new(AtomicUsize::new(rows)),
        }
    }

    fn detect_size() -> (usize, usize) {
        crossterm::terminal::size()
            .map(|(cols, rows)| (cols as usize, rows as usize))
            .unwrap_or((80, 24))
    }

    fn set_raw_mode(&self) -> Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(())
    }

    fn unset_raw_mode(&self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal for ProcessTerminal {
    fn start(&mut self, on_input: Box<dyn Fn(String) + Send>, on_resize: Box<dyn Fn() + Send>) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        self.set_raw_mode()?;

        let running = self.running.clone();
        let columns = self.columns.clone();
        let rows = self.rows.clone();

        // Input reader thread
        thread::spawn(move || {
            let stdin = stdin();
            let mut reader = stdin.lock();
            let mut buffer = [0u8; 1024];

            while running.load(Ordering::SeqCst) {
                if let Ok(len) = reader.read(&mut buffer) {
                    if len > 0 {
                        let input = String::from_utf8_lossy(&buffer[..len]).to_string();
                        on_input(input);
                    }
                }
            }
        });

        // Initial resize detection
        let initial_cols = columns.clone();
        let initial_rows = rows.clone();
        let (init_cols, init_rows) = Self::detect_size();
        initial_cols.store(init_cols, Ordering::SeqCst);
        initial_rows.store(init_rows, Ordering::SeqCst);

        // Resize handler (check periodically)
        let columns_clone = columns.clone();
        let rows_clone = rows.clone();
        let running_clone = running.clone();
        thread::spawn(move || {
            let mut last_cols = columns_clone.load(Ordering::SeqCst);
            let mut last_rows = rows_clone.load(Ordering::SeqCst);

            while running_clone.load(Ordering::SeqCst) {
                thread::sleep(std::time::Duration::from_millis(500));
                let (new_cols, new_rows) = Self::detect_size();

                if new_cols != last_cols || new_rows != last_rows {
                    columns_clone.store(new_cols, Ordering::SeqCst);
                    rows_clone.store(new_rows, Ordering::SeqCst);
                    last_cols = new_cols;
                    last_rows = new_rows;
                    on_resize();
                }
            }
        });

        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.unset_raw_mode();
    }

    fn write(&self, data: &str) {
        print!("{}", data);
        let _ = stdout().flush();
    }

    fn columns(&self) -> usize {
        self.columns.load(Ordering::SeqCst)
    }

    fn rows(&self) -> usize {
        self.rows.load(Ordering::SeqCst)
    }

    fn hide_cursor(&self) {
        let _ = execute!(stdout(), cursor::Hide);
        let _ = stdout().flush();
    }

    fn show_cursor(&self) {
        let _ = execute!(stdout(), cursor::Show);
        let _ = stdout().flush();
    }

    fn clear_line(&self) {
        let _ = execute!(stdout(), terminal::Clear(ClearType::CurrentLine));
        let _ = stdout().flush();
    }

    fn clear_screen(&self) {
        let _ = execute!(stdout(), terminal::Clear(ClearType::All));
        let _ = stdout().flush();
    }
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        self.stop();
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;