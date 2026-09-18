//! The screen's state and the events that change it.
//!
//! A pure reducer: [`App::update`] takes an [`Event`] and mutates the [`App`].
//! Nothing here touches the terminal or the host, which is what makes the
//! behaviour testable without either.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// One company as the screen lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanyRow {
    pub id: String,
    /// Whether a turn is in flight right now.
    pub busy: bool,
}

/// Where the host got to.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum HostStatus {
    #[default]
    Starting,
    Ready {
        address: String,
        instance_id: String,
        home: String,
    },
    Failed(String),
}

/// Everything that can happen to the screen.
#[derive(Clone, Debug)]
pub enum Event {
    Key(KeyEvent),
    HostReady {
        address: String,
        instance_id: String,
        home: String,
        companies: Vec<CompanyRow>,
    },
    HostFailed(String),
    /// A fresh view of the registry.
    Companies(Vec<CompanyRow>),
    Tick,
    Log(String),
}

/// How many log lines the screen keeps. Older ones scroll off; the file log
/// keeps everything.
const LOG_CAPACITY: usize = 200;

#[derive(Debug, Default)]
pub struct App {
    pub host: HostStatus,
    pub companies: Vec<CompanyRow>,
    /// Index into `companies`; always in bounds when the list is non-empty.
    pub selected: usize,
    pub log: Vec<String>,
    pub should_quit: bool,
}

impl App {
    pub fn update(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::HostReady {
                address,
                instance_id,
                home,
                companies,
            } => {
                self.push_log(format!("host listening on {address}"));
                self.host = HostStatus::Ready {
                    address,
                    instance_id,
                    home,
                };
                self.set_companies(companies);
            }
            Event::HostFailed(reason) => {
                self.push_log(format!("host failed to start: {reason}"));
                self.host = HostStatus::Failed(reason);
            }
            Event::Companies(companies) => self.set_companies(companies),
            Event::Tick => {}
            Event::Log(line) => self.push_log(line),
        }
    }

    /// The company under the cursor, if any.
    pub fn selected_company(&self) -> Option<&CompanyRow> {
        self.companies.get(self.selected)
    }

    fn on_key(&mut self, key: KeyEvent) {
        // A terminal reports both press and release on some platforms; acting
        // on both would move the cursor twice per keystroke.
        if key.kind != KeyEventKind::Press {
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => self.should_quit = true,
            (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => self.should_quit = true,
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => self.move_selection(1),
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => self.move_selection(-1),
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.companies.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.companies.len() - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last as isize) as usize;
    }

    fn set_companies(&mut self, companies: Vec<CompanyRow>) {
        self.companies = companies;
        if self.companies.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.companies.len() - 1);
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > LOG_CAPACITY {
            let excess = self.log.len() - LOG_CAPACITY;
            self.log.drain(..excess);
        }
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
