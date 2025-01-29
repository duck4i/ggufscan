use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    fs,
    io::{self, stdout},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};
use walkdir::WalkDir;

#[derive(Debug)]
struct FileInfo {
    path: PathBuf,
    size: u64,
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
    let walker = WalkDir::new("/")
        .follow_links(true)
        .same_file_system(true)
        .into_iter();

    for entry in walker {
        match entry {
            Ok(entry) => {
                let path = entry.path();

                // Send directory updates for folders
                if path.is_dir() {
                    if let Some(path_str) = path.to_str() {
                        tx.send(ScanMessage::Directory(path_str.to_string())).ok();
                    }
                }

                // Send file updates for .gguf files
                if path.is_file() && path.extension().map_or(false, |ext| ext == "gguf") {
                    if let Ok(metadata) = fs::metadata(path) {
                        tx.send(ScanMessage::File(FileInfo {
                            path: path.to_owned(),
                            size: metadata.len(),
                        }))
                        .ok();
                    }
                }
            }
            Err(err) => {
                tx.send(ScanMessage::Error(err.to_string())).ok();
                continue;
            }
        }

        // Small sleep to prevent overwhelming the UI
        thread::sleep(Duration::from_millis(1));
    }

    tx.send(ScanMessage::Done).ok();
}

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
            "Directories: {} | Files found: {} | Scanning... {}",
            app.dirs_scanned, app.files_found, app.current_path
        )
    } else {
        format!("Scan complete | Found {} .gguf files", app.files.len())
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
        // Process any pending scan messages
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
                    ScanMessage::Error(_) => {
                        // Optionally handle errors
                    }
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

    // Spawn scanning thread
    thread::spawn(move || {
        scan_directory(tx);
    });

    // Run the UI with the receiver
    run_app(rx).context("Error running application")?;

    Ok(())
}
