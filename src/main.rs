use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ignore::WalkBuilder;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use std::{
    fs,
    io::{self, stdout, Read},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

const GGUF_MAGIC: &[u8] = b"GGUF";

#[derive(Debug)]
struct FileInfo {
    path: PathBuf,
    size: u64,
}

// Function to check if a file is a GGUF file by reading its magic number
fn is_gguf_file(path: &std::path::Path) -> io::Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 4];

    match file.read_exact(&mut buffer) {
        Ok(_) => Ok(buffer == GGUF_MAGIC),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

struct App {
    files: Vec<FileInfo>,
    selected: Vec<bool>,
    list_state: ListState,
    scanning: bool,
    current_path: String,
    dirs_scanned: usize,
    files_found: usize,
}

impl App {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            selected: Vec::new(),
            list_state: ListState::default(),
            scanning: true,
            current_path: String::new(),
            dirs_scanned: 0,
            files_found: 0,
        }
    }

    fn toggle_selected(&mut self) {
        if let Some(i) = self.list_state.selected() {
            self.selected[i] = !self.selected[i];
        }
    }

    fn select_all(&mut self) {
        for selected in self.selected.iter_mut() {
            *selected = true;
        }
    }

    fn deselect_all(&mut self) {
        for selected in self.selected.iter_mut() {
            *selected = false;
        }
    }

    fn next(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.files.len().saturating_sub(1) {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.files.len().saturating_sub(1)
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn delete_selected(&mut self) -> io::Result<()> {
        let mut i = 0;
        while i < self.files.len() {
            if self.selected[i] {
                fs::remove_file(&self.files[i].path)?;
                self.files.remove(i);
                self.selected.remove(i);
            } else {
                i += 1;
            }
        }
        if let Some(selected) = self.list_state.selected() {
            if selected >= self.files.len() {
                self.list_state
                    .select(Some(self.files.len().saturating_sub(1)));
            }
        }
        Ok(())
    }

    fn get_selected_size(&self) -> u64 {
        self.files
            .iter()
            .zip(self.selected.iter())
            .filter(|(_, &selected)| selected)
            .map(|(file, _)| file.size)
            .sum()
    }
}

#[derive(Debug)]
enum ScanMessage {
    File(FileInfo),
    Directory(String),
    Done,
    Error(String),
}

fn format_size(size: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;

    if size >= GB {
        format!("{:.2} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.2} MB", size as f64 / MB as f64)
    } else {
        format!("{} B", size)
    }
}

fn scan_directory(tx: Sender<ScanMessage>) {
    let (worker_tx, worker_rx) = mpsc::channel();
    let tx_clone = tx.clone();

    thread::spawn(move || {
        for message in worker_rx {
            tx_clone.send(message).ok();
        }
    });

    let walker = WalkBuilder::new("/")
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .threads(num_cpus::get())
        .build_parallel();

    walker.run(|| {
        let worker_tx = worker_tx.clone();
        Box::new(move |entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return ignore::WalkState::Continue,
            };

            let path = entry.path();

            // Send directory updates
            if path.is_dir() {
                if let Some(path_str) = path.to_str() {
                    worker_tx
                        .send(ScanMessage::Directory(path_str.to_string()))
                        .ok();
                }
            }

            // Check if it's a file and has the GGUF magic number
            if path.is_file() {
                match is_gguf_file(path) {
                    Ok(true) => {
                        if let Ok(metadata) = fs::metadata(path) {
                            worker_tx
                                .send(ScanMessage::File(FileInfo {
                                    path: path.to_owned(),
                                    size: metadata.len(),
                                }))
                                .ok();
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        worker_tx
                            .send(ScanMessage::Error(format!(
                                "Error reading file {}: {}",
                                path.display(),
                                e
                            )))
                            .ok();
                    }
                }
            }

            ignore::WalkState::Continue
        })
    });

    tx.send(ScanMessage::Done).ok();
}

// UI code and run_app function remain the same...
fn ui(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let title = if app.scanning {
        format!(
            "Scanning: {} | Directories: {} | Files found: {}",
            app.current_path, app.dirs_scanned, app.files_found
        )
    } else {
        format!("Scan complete | Found {} GGUF files", app.files.len())
    };

    frame.render_widget(
        Paragraph::new(title)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let items: Vec<ListItem> = app
        .files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let checkbox = if app.selected[i] { "[x] " } else { "[ ] " };
            ListItem::new(format!(
                "{}{:<10} | {}",
                checkbox,
                format_size(file.size),
                file.path.display()
            ))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title("Files").borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_stateful_widget(list, chunks[1], &mut app.list_state);

    let total_selected_size = format_size(app.get_selected_size());
    let help_text = format!(
        "↑/↓: Navigate | Space: Toggle | A: Select All | U: Deselect All | D: Delete Selected | Q: Quit | Selected size: {}",
        total_selected_size
    );

    frame.render_widget(
        Paragraph::new(help_text)
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center),
        chunks[2],
    );
}

fn run_app(rx: Receiver<ScanMessage>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut app = App::new();

    loop {
        if app.scanning {
            while let Ok(message) = rx.try_recv() {
                match message {
                    ScanMessage::File(file_info) => {
                        app.files.push(file_info);
                        app.selected.push(false);
                        app.files_found += 1;
                        if app.files.len() == 1 {
                            app.list_state.select(Some(0));
                        }
                    }
                    ScanMessage::Directory(path) => {
                        app.current_path = path;
                        app.dirs_scanned += 1;
                    }
                    ScanMessage::Done => {
                        app.scanning = false;
                    }
                    ScanMessage::Error(_) => {}
                }
            }
        }

        terminal.draw(|frame| ui(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Up => app.previous(),
                    KeyCode::Down => app.next(),
                    KeyCode::Char(' ') => app.toggle_selected(),
                    KeyCode::Char('a') => app.select_all(),
                    KeyCode::Char('u') => app.deselect_all(),
                    KeyCode::Char('d') => app.delete_selected()?,
                    _ => {}
                },
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

    Ok(())
}

fn main() -> Result<()> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        scan_directory(tx);
    });

    run_app(rx).context("Error running application")?;

    Ok(())
}
