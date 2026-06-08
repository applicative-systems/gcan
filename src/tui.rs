//! Interactive terminal UI: browse, sort, toggle, and delete GC roots.

use crate::cache::{self, Cache};
use crate::format::{human_age, iec_size};
use crate::output::{Row, SortKey, order};
use crate::{size, walk};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row as TRow, Table, TableState};
use std::io;
use std::path::{Path, PathBuf};

struct App {
    dir: PathBuf,
    cache: Cache,
    use_cache: bool,
    now: u64,
    rows: Vec<Row>,
    /// Indices into `rows`, filtered and sorted — the visible order.
    view: Vec<usize>,
    selected: usize,
    sort: SortKey,
    desc: bool,
    show_all: bool,
    /// Persistent predicate floor for the session.
    min_size: u64,
    min_age: u64,
    /// Index into `rows` of a delete awaiting confirmation.
    confirm: Option<usize>,
    status: String,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        groups: Vec<walk::Group>,
        mut cache: Cache,
        now: u64,
        dir: &Path,
        use_cache: bool,
        show_all: bool,
        min_size: u64,
        min_age: u64,
        sort: SortKey,
    ) -> App {
        let rows = resolve_rows(&groups, &mut cache, now);
        let mut app = App {
            dir: dir.to_path_buf(),
            cache,
            use_cache,
            now,
            rows,
            view: Vec::new(),
            selected: 0,
            sort,
            desc: sort.default_desc(),
            show_all,
            min_size,
            min_age,
            confirm: None,
            status: String::from(
                "↑/↓ move · s/n/a sort · r reverse · t toggle · D delete · q quit",
            ),
        };
        app.rebuild_view();
        app
    }

    /// Re-walk the gcroots tree and recompute sizes (after a deletion).
    fn rescan(&mut self) {
        let groups = walk::scan(&self.dir);
        self.rows = resolve_rows(&groups, &mut self.cache, self.now);
        if self.use_cache {
            let _ = self.cache.save(&cache::default_path());
        }
        self.rebuild_view();
    }

    /// Recompute the filtered, sorted view and clamp the selection.
    fn rebuild_view(&mut self) {
        let (sort, desc, show_all) = (self.sort, self.desc, self.show_all);
        let (min_size, min_age) = (self.min_size, self.min_age);
        let rows = &self.rows;
        let mut view: Vec<usize> = (0..rows.len())
            .filter(|&i| {
                let r = &rows[i];
                (show_all || r.deletable) && r.size >= min_size && r.age >= min_age
            })
            .collect();
        view.sort_by(|&a, &b| order(&rows[a], &rows[b], sort, desc));
        self.view = view;
        if self.selected >= self.view.len() {
            self.selected = self.view.len().saturating_sub(1);
        }
    }

    fn total_reclaimable(&self) -> u64 {
        self.rows
            .iter()
            .filter(|r| r.deletable)
            .map(|r| r.size)
            .sum()
    }

    fn set_sort(&mut self, sort: SortKey) {
        if self.sort == sort {
            self.desc = !self.desc;
        } else {
            self.sort = sort;
            self.desc = sort.default_desc();
        }
        self.rebuild_view();
    }

    fn selected_row(&self) -> Option<usize> {
        self.view.get(self.selected).copied()
    }

    /// Perform the confirmed deletion of `rows[idx]`'s symlinks.
    fn do_delete(&mut self, idx: usize) {
        let links = self.rows[idx].links.clone();
        let mut removed = 0usize;
        for l in &links {
            if std::fs::remove_file(l).is_ok() {
                removed += 1;
            }
        }
        self.rescan();
        self.status = format!(
            "Removed {removed}/{} symlink(s). Run nix-collect-garbage to reclaim the space.",
            links.len()
        );
    }
}

/// Compute a display row for every group (deletable or not), using the cache.
fn resolve_rows(groups: &[walk::Group], cache: &mut Cache, now: u64) -> Vec<Row> {
    let refs: Vec<&walk::Group> = groups.iter().collect();
    let sizes = size::group_sizes(&refs, cache);
    refs.iter()
        .zip(&sizes)
        .map(|(g, &sz)| Row::from_group(g, sz, now))
        .collect()
}

/// Entry point: own the data, run the event loop, restore the terminal.
#[allow(clippy::too_many_arguments)]
pub fn run(
    groups: Vec<walk::Group>,
    cache: Cache,
    now: u64,
    dir: &Path,
    use_cache: bool,
    show_all: bool,
    min_size: u64,
    min_age: u64,
    sort: SortKey,
) -> io::Result<()> {
    eprintln!("scanning GC roots…");
    let mut app = App::new(
        groups, cache, now, dir, use_cache, show_all, min_size, min_age, sort,
    );

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();

    if app.use_cache {
        let _ = app.cache.save(&cache::default_path());
    }
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| ui(frame, app))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Modal: only y / n / esc are live while confirming a delete.
        if let Some(idx) = app.confirm {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.confirm = None;
                    app.do_delete(idx);
                }
                _ => {
                    app.confirm = None;
                    app.status = String::from("cancelled");
                }
            }
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Down | KeyCode::Char('j') => move_sel(app, 1),
            KeyCode::Up | KeyCode::Char('k') => move_sel(app, -1),
            KeyCode::Home => app.selected = 0,
            KeyCode::End => app.selected = app.view.len().saturating_sub(1),
            KeyCode::Char('s') => app.set_sort(SortKey::Size),
            KeyCode::Char('n') => app.set_sort(SortKey::Name),
            KeyCode::Char('a') => app.set_sort(SortKey::Age),
            KeyCode::Char('r') => {
                app.desc = !app.desc;
                app.rebuild_view();
            }
            KeyCode::Char('t') => {
                app.show_all = !app.show_all;
                app.rebuild_view();
            }
            KeyCode::Char('d') | KeyCode::Char('D') => request_delete(app),
            _ => {}
        }
    }
}

fn move_sel(app: &mut App, delta: isize) {
    if app.view.is_empty() {
        return;
    }
    let last = app.view.len() - 1;
    let cur = app.selected as isize;
    app.selected = cur.saturating_add(delta).clamp(0, last as isize) as usize;
}

fn request_delete(app: &mut App) {
    match app.selected_row() {
        None => app.status = String::from("nothing to delete"),
        Some(idx) if !app.rows[idx].deletable => {
            app.status = if app.rows[idx].protected {
                String::from("cannot delete: protected (current/booted) root")
            } else {
                String::from("cannot delete: not owned by you (root-owned)")
            };
        }
        Some(idx) => app.confirm = Some(idx),
    }
}

fn ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(0),    // table
        Constraint::Length(1), // status
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    title(frame, chunks[0], app);
    table(frame, chunks[1], app);
    frame.render_widget(
        Paragraph::new(app.status.as_str()).style(Style::new().fg(Color::Yellow)),
        chunks[2],
    );
    frame.render_widget(
        Paragraph::new(
            "↑/↓ move   s size   n name   a age   r reverse   t toggle all   D delete   q quit",
        )
        .style(Style::new().fg(Color::DarkGray)),
        chunks[3],
    );

    if app.confirm.is_some() {
        confirm_modal(frame, app);
    }
}

fn title(frame: &mut Frame, area: Rect, app: &App) {
    let arrow = if app.desc { "↓" } else { "↑" };
    let showing = if app.show_all { "all" } else { "deletable" };
    let line = Line::from(vec![
        Span::styled("gcan", Style::new().fg(Color::Cyan).bold()),
        Span::raw(format!(
            "  ·  {} shown / {} roots  ·  {} reclaimable  ·  sort: {}{}  ·  showing: {}",
            app.view.len(),
            app.rows.len(),
            iec_size(app.total_reclaimable()),
            app.sort.label(),
            arrow,
            showing,
        )),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn table(frame: &mut Frame, area: Rect, app: &App) {
    let header = TRow::new([
        cell_r("SIZE"),
        cell_r("AGE"),
        cell_r("ROOTS"),
        Cell::from("LOCATION"),
    ])
    .style(Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

    let rows = app.view.iter().map(|&i| {
        let r = &app.rows[i];
        let mut loc = r.loc.clone();
        if !r.deletable {
            loc.push_str(if r.protected {
                "  [protected]"
            } else {
                "  [root-owned]"
            });
        }
        let style = if r.deletable {
            Style::new()
        } else {
            Style::new().fg(Color::DarkGray)
        };
        TRow::new([
            cell_r(&iec_size(r.size)),
            cell_r(&human_age(r.age)),
            cell_r(&r.count.to_string()),
            Cell::from(loc),
        ])
        .style(style)
    });

    let widths = [
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("» ");

    let mut state = TableState::default();
    if !app.view.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn cell_r(s: &str) -> Cell<'static> {
    Cell::from(Line::from(s.to_string()).alignment(Alignment::Right))
}

fn confirm_modal(frame: &mut Frame, app: &App) {
    let Some(idx) = app.confirm else { return };
    let r = &app.rows[idx];
    let area = centered(frame.area(), 64, 7);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" confirm delete ")
        .border_style(Style::new().fg(Color::Red));
    let text = vec![
        Line::from(format!("Delete {} symlink(s) for:", r.links.len())),
        Line::from(Span::styled(r.loc.clone(), Style::new().bold())),
        Line::from(format!("({} reclaimable)", iec_size(r.size))),
        Line::from(""),
        Line::from(Span::styled(
            "y = delete    any other key = cancel",
            Style::new().fg(Color::Yellow),
        )),
    ];
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}
