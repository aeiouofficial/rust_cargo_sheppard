// src/tui.rs
// cargo-shepherd interactive TUI dashboard.
//
// Layout:
//   ┌── header: title + resource gauges + slot counter ──────────────────────┐
//   │   RUNNING [n/slots]        │  QUEUE [n waiting]                        │
//   │   ● alias  cargo build     │  [CRIT] 1. alias   cargo run              │
//   │     PID: 1234  00:23       │  [HIGH] 2. alias   cargo check            │
//   │   ● alias  cargo check     │  [NORM] 3. alias   cargo test             │
//   │                            │                                            │
//   ├── status bar ──────────────────────────────────────────────────────────┤
//   │  j/k Navigate  +/- Priority  x Kill  c Cancel  s Slots  a Alias  q Quit│
//   └────────────────────────────────────────────────────────────────────────┘
//
// Keyboard bindings:
//   j / ↓      select next item in queue
//   k / ↑      select previous item in queue
//   Tab        switch focus between RUNNING and QUEUE panels
//   + / =      raise priority of selected queued job
//   -          lower priority of selected queued job
//   x          kill selected job (either panel)
//   c          cancel selected queued job (if not yet running)
//   s          open slot-count prompt
//   a          open alias prompt for selected job's project
//   r          force refresh now
//   q / Esc    quit TUI (daemon keeps running)

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;
use tokio::time::interval;

use crate::client::ShepherdClient;
use crate::config::Priority;
use crate::ipc::{ClientMsg, DaemonMsg, QueuedJobSnapshot, RunningJob, StatusReport};

// ── Focus ─────────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Panel { Running, Queue }

// ── Input mode ────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum InputMode {
    Normal,
    SlotInput,
    AliasInput,
}

// ── App state ─────────────────────────────────────────────────────────────────

struct App {
    report:          StatusReport,
    focus:           Panel,
    running_sel:     ListState,
    queue_sel:       ListState,
    input_mode:      InputMode,
    input_buf:       String,
    status_msg:      String,             // one-line feedback in the footer
    daemon_connected: bool,
}

impl App {
    fn new() -> Self {
        let mut queue_sel = ListState::default();
        queue_sel.select(Some(0));

        Self {
            report:           StatusReport::empty(),
            focus:            Panel::Queue,
            running_sel:      ListState::default(),
            queue_sel,
            input_mode:       InputMode::Normal,
            input_buf:        String::new(),
            status_msg:       String::from("Connected — press ? for help"),
            daemon_connected: false,
        }
    }

    fn selected_queued(&self) -> Option<&QueuedJobSnapshot> {
        let idx = self.queue_sel.selected()?;
        self.report.queued.get(idx)
    }

    fn selected_running(&self) -> Option<&RunningJob> {
        let idx = self.running_sel.selected()?;
        self.report.running.get(idx)
    }

    fn clamp_selections(&mut self) {
        let qlen = self.report.queued.len();
        if qlen == 0 {
            self.queue_sel.select(None);
        } else if let Some(i) = self.queue_sel.selected() {
            if i >= qlen { self.queue_sel.select(Some(qlen - 1)); }
        } else {
            self.queue_sel.select(Some(0));
        }

        let rlen = self.report.running.len();
        if rlen == 0 {
            self.running_sel.select(None);
        } else if let Some(i) = self.running_sel.selected() {
            if i >= rlen { self.running_sel.select(Some(rlen - 1)); }
        } else {
            self.running_sel.select(Some(0));
        }
    }

    fn nav_down(&mut self) {
        match self.focus {
            Panel::Queue => {
                let len = self.report.queued.len();
                if len == 0 { return; }
                let next = self.queue_sel.selected().map(|i| (i + 1).min(len - 1)).unwrap_or(0);
                self.queue_sel.select(Some(next));
            }
            Panel::Running => {
                let len = self.report.running.len();
                if len == 0 { return; }
                let next = self.running_sel.selected().map(|i| (i + 1).min(len - 1)).unwrap_or(0);
                self.running_sel.select(Some(next));
            }
        }
    }

    fn nav_up(&mut self) {
        match self.focus {
            Panel::Queue => {
                let next = self.queue_sel.selected().map(|i| i.saturating_sub(1)).unwrap_or(0);
                self.queue_sel.select(Some(next));
            }
            Panel::Running => {
                let next = self.running_sel.selected().map(|i| i.saturating_sub(1)).unwrap_or(0);
                self.running_sel.select(Some(next));
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run_tui(refresh_ms: u64) -> Result<()> {
    let mut terminal = ratatui::init();
    let result       = tui_loop(&mut terminal, refresh_ms).await;
    ratatui::restore();

    if let Err(e) = &result {
        eprintln!("TUI error: {}", e);
    }
    result
}

// ── Main event loop ───────────────────────────────────────────────────────────

async fn tui_loop(terminal: &mut ratatui::DefaultTerminal, refresh_ms: u64) -> Result<()> {
    let mut app        = App::new();
    let mut events     = EventStream::new();
    let mut poll_timer = interval(Duration::from_millis(refresh_ms));

    // Initial fetch
    fetch_status(&mut app).await;

    loop {
        terminal.draw(|frame| render(frame, &mut app))?;

        tokio::select! {
            _ = poll_timer.tick() => {
                if app.input_mode == InputMode::Normal {
                    fetch_status(&mut app).await;
                }
            }

            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if handle_key(&mut app, key).await? {
                            break; // quit
                        }
                    }
                    Some(Err(e)) => return Err(e.into()),
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

// ── Key handling ──────────────────────────────────────────────────────────────

/// Returns true if the user wants to quit.
async fn handle_key(
    app:      &mut App,
    key:      crossterm::event::KeyEvent,
) -> Result<bool> {
    match app.input_mode {
        InputMode::Normal => handle_normal_key(app, key).await,
        InputMode::SlotInput => {
            handle_text_input(app, key, |app, s| {
                Box::pin(async move { submit_slots(app, s).await })
            }).await
        }
        InputMode::AliasInput => {
            handle_text_input(app, key, |app, s| {
                Box::pin(async move { submit_alias(app, s).await })
            }).await
        }
    }
}

async fn handle_normal_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
) -> Result<bool> {
    match key.code {
        // ── Quit ─────────────────────────────────────────────────────────────
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),

        // ── Navigation ───────────────────────────────────────────────────────
        KeyCode::Char('j') | KeyCode::Down  => app.nav_down(),
        KeyCode::Char('k') | KeyCode::Up    => app.nav_up(),
        KeyCode::Tab => {
            app.focus = if app.focus == Panel::Queue { Panel::Running } else { Panel::Queue };
        }

        // ── Priority raise ────────────────────────────────────────────────────
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if app.focus == Panel::Queue {
                if let Some(job) = app.selected_queued() {
                    let new_p = job.priority.raised();
                    let job_id = job.job_id.clone();
                    send_daemon(app, &ClientMsg::SetJobPriority { job_id, new_priority: new_p }).await;
                }
            }
        }

        // ── Priority lower ────────────────────────────────────────────────────
        KeyCode::Char('-') => {
            if app.focus == Panel::Queue {
                if let Some(job) = app.selected_queued() {
                    let new_p = job.priority.lowered();
                    let job_id = job.job_id.clone();
                    send_daemon(app, &ClientMsg::SetJobPriority { job_id, new_priority: new_p }).await;
                }
            }
        }

        // ── Kill (both panels) ────────────────────────────────────────────────
        KeyCode::Char('x') => {
            let msg = match app.focus {
                Panel::Queue => app.selected_queued().map(|j| ClientMsg::KillJob { job_id: j.job_id.clone() }),
                Panel::Running => app.selected_running().map(|j| ClientMsg::KillJob { job_id: j.job_id.clone() }),
            };
            if let Some(m) = msg {
                send_daemon(app, &m).await;
            }
        }

        // ── Cancel queued job ─────────────────────────────────────────────────
        KeyCode::Char('c') => {
            if app.focus == Panel::Queue {
                if let Some(job) = app.selected_queued() {
                    let job_id = job.job_id.clone();
                    send_daemon(app, &ClientMsg::CancelJob { job_id }).await;
                }
            }
        }

        // ── Kill entire project ───────────────────────────────────────────────
        KeyCode::Char('X') => {
            let project_dir = match app.focus {
                Panel::Queue   => app.selected_queued().map(|j| j.project_dir.clone()),
                Panel::Running => app.selected_running().map(|j| j.project_dir.clone()),
            };
            if let Some(dir) = project_dir {
                send_daemon(app, &ClientMsg::KillProject { project_dir: dir }).await;
            }
        }

        // ── Set slots prompt ──────────────────────────────────────────────────
        KeyCode::Char('s') => {
            let current = app.report.slots_total;
            app.input_buf   = current.to_string();
            app.input_mode  = InputMode::SlotInput;
            app.status_msg  = format!("Set slots (current: {}) — Enter to confirm, Esc to cancel", current);
        }

        // ── Set alias prompt ──────────────────────────────────────────────────
        KeyCode::Char('a') => {
            let current_alias = match app.focus {
                Panel::Queue   => app.selected_queued().map(|j| j.alias.clone()),
                Panel::Running => app.selected_running().map(|j| j.alias.clone()),
            };
            if let Some(alias) = current_alias {
                app.input_buf  = alias;
                app.input_mode = InputMode::AliasInput;
                app.status_msg = "Set alias — Enter to confirm, Esc to cancel".into();
            } else {
                app.status_msg = "Select a job first".into();
            }
        }

        // ── Force refresh ─────────────────────────────────────────────────────
        KeyCode::Char('r') => {
            fetch_status(app).await;
            app.status_msg = "Refreshed".into();
        }

        _ => {}
    }

    Ok(false)
}

/// Generic text-input handler — calls `on_submit(app, text)` on Enter.
async fn handle_text_input<F, Fut>(
    app:       &mut App,
    key:       crossterm::event::KeyEvent,
    on_submit: F,
) -> Result<bool>
where
    F: FnOnce(&mut App, String) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    match key.code {
        KeyCode::Enter => {
            let text       = app.input_buf.clone();
            app.input_mode = InputMode::Normal;
            on_submit(app, text).await
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.input_buf  = String::new();
            app.status_msg = "Cancelled".into();
            Ok(false)
        }
        KeyCode::Backspace => {
            app.input_buf.pop();
            Ok(false)
        }
        KeyCode::Char(c) => {
            app.input_buf.push(c);
            Ok(false)
        }
        _ => Ok(false),
    }
}

async fn submit_slots(app: &mut App, s: String) -> Result<bool> {
    match s.trim().parse::<usize>() {
        Ok(n) if n > 0 => {
            send_daemon(app, &ClientMsg::SetSlots { slots: n }).await;
        }
        _ => {
            app.status_msg = format!("Invalid slot count: '{}' — must be a positive integer", s);
        }
    }
    Ok(false)
}

async fn submit_alias(app: &mut App, alias: String) -> Result<bool> {
    let project_dir = match app.focus {
        Panel::Queue   => app.selected_queued().map(|j| j.project_dir.clone()),
        Panel::Running => app.selected_running().map(|j| j.project_dir.clone()),
    };
    if let Some(dir) = project_dir {
        send_daemon(app, &ClientMsg::SetProjectAlias {
            project_dir: dir,
            alias: alias.trim().to_string(),
        }).await;
    }
    Ok(false)
}

// ── Daemon helpers ────────────────────────────────────────────────────────────

async fn fetch_status(app: &mut App) {
    match ShepherdClient::connect().await {
        Ok(mut client) => {
            app.daemon_connected = true;
            match client.send_recv(&ClientMsg::Status).await {
                Ok(DaemonMsg::StatusReport { report }) => {
                    app.report = report;
                    app.clamp_selections();
                }
                Ok(DaemonMsg::Error { message }) => {
                    app.status_msg = format!("Daemon error: {}", message);
                }
                Err(e) => {
                    app.status_msg = format!("Fetch error: {}", e);
                }
                _ => {}
            }
        }
        Err(_) => {
            app.daemon_connected = false;
            app.report           = StatusReport::empty();
            app.status_msg       = "⚠ Daemon not reachable — run: shepherd daemon".into();
        }
    }
}

async fn send_daemon(app: &mut App, msg: &ClientMsg) {
    match ShepherdClient::connect().await {
        Ok(mut client) => {
            match client.send_recv(msg).await {
                Ok(DaemonMsg::ConfigUpdated { message }) => app.status_msg = format!("✔ {}", message),
                Ok(DaemonMsg::Killed        { description }) => app.status_msg = format!("✔ {}", description),
                Ok(DaemonMsg::PriorityChanged { new_priority, new_position, .. }) => {
                    app.status_msg = format!("✔ Priority → {}  (position {})", new_priority, new_position + 1);
                }
                Ok(DaemonMsg::Error { message }) => app.status_msg = format!("✗ {}", message),
                Err(e) => app.status_msg = format!("✗ {}", e),
                _ => {}
            }
            // Immediately refresh so the UI reflects the change
            fetch_status(app).await;
        }
        Err(e) => {
            app.status_msg = format!("✗ Cannot reach daemon: {}", e);
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // ── Outer vertical split: header / body / footer ──────────────────────────
    let [header_area, body_area, footer_area] = *Layout::vertical([
        Constraint::Length(10),  // header: logo + title + gauges
        Constraint::Min(0),      // body:   running | queue
        Constraint::Length(3),   // footer: keybindings + status
    ]).split(area) else { return };

    render_header(frame, app, header_area);
    render_body(frame, app, body_area);
    render_footer(frame, app, footer_area);
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let [title_area, gauges_area] = *Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(3),
    ]).split(area) else { return };

    // ── Title bar ─────────────────────────────────────────────────────────────
    let conn_indicator = if app.daemon_connected {
        Span::styled(" ● LIVE ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" ○ OFFLINE ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    };

    let slots_text = format!(
        " Slots: {}/{} ",
        app.report.slots_active,
        app.report.slots_total
    );

    let logo_ascii = [
        "  ____  _                      _               _ ",
        " / ___|| |__   ___ _ __  _ __ | |__   ___ _ __| |",
        " \\___ \\| '_ \\ / _ \\ '_ \\| '_ \\| '_ \\ / _ \\ '__| |",
        "  ___) | | | |  __/ |_) | |_) | | | |  __/ |  |_|",
        " |____/|_| |_|\\___| .__/| .__/|_| |_|\\___|_|  (_)",
        "                  |_|   |_|                      ",
    ];

    let header_layout = Layout::horizontal([
        Constraint::Length(52), // Logo width
        Constraint::Min(0),      // Info
    ]).split(title_area);

    let logo_p = Paragraph::new(logo_ascii.join("\n"))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    frame.render_widget(logo_p, header_layout[0]);

    let info_lines = vec![
        Line::from(vec![
            Span::styled(" 🐑 v0.2.0 ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            conn_indicator,
        ]),
        Line::from(vec![
            Span::styled(slots_text, Style::default().fg(Color::White)),
        ]),
    ];
    frame.render_widget(Paragraph::new(info_lines), header_layout[1]);

    // ── Resource gauges ───────────────────────────────────────────────────────
    let [cpu_area, ram_area] = *Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ]).split(gauges_area) else { return };

    let cpu_pct = app.report.cpu_pct as u16;
    let ram_pct = app.report.ram_pct as u16;

    let cpu_color = if cpu_pct >= 80 { Color::Red } else if cpu_pct >= 60 { Color::Yellow } else { Color::Green };
    let ram_color = if ram_pct >= 85 { Color::Red } else if ram_pct >= 65 { Color::Yellow } else { Color::Green };

    frame.render_widget(
        Gauge::default()
            .block(Block::default().title(" CPU ").borders(Borders::ALL).border_type(BorderType::Rounded))
            .gauge_style(Style::default().fg(cpu_color))
            .percent(cpu_pct.min(100))
            .label(format!("{}%", cpu_pct)),
        cpu_area,
    );

    frame.render_widget(
        Gauge::default()
            .block(Block::default().title(" RAM ").borders(Borders::ALL).border_type(BorderType::Rounded))
            .gauge_style(Style::default().fg(ram_color))
            .percent(ram_pct.min(100))
            .label(format!("{}%", ram_pct)),
        ram_area,
    );
}

fn render_body(frame: &mut Frame, app: &mut App, area: Rect) {
    let [running_area, queue_area] = *Layout::horizontal([
        Constraint::Percentage(45),
        Constraint::Percentage(55),
    ]).split(area) else { return };

    render_running_panel(frame, app, running_area);
    render_queue_panel(frame, app, queue_area);
}

fn render_running_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focus == Panel::Running;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = format!(" RUNNING [{}/{}] ", app.report.slots_active, app.report.slots_total);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    if app.report.running.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  No active builds")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app.report.running.iter().map(|job| {
        let elapsed = format_elapsed(job.elapsed_ms);
        let cmd     = format!("cargo {}", job.args.join(" "));
        let pid     = if job.pid > 0 { format!("PID:{}", job.pid) } else { "PID:?".into() };

        ListItem::new(vec![
            Line::from(vec![
                Span::styled("● ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::styled(truncate(&job.alias, 18), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(truncate(&cmd, 28), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled(format!("  {} · {}", pid, elapsed), Style::default().fg(Color::DarkGray)),
            ]),
        ])
    }).collect();

    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ "),
        area,
        &mut app.running_sel,
    );
}

fn render_queue_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focus == Panel::Queue;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = format!(" QUEUE [{}] ", app.report.queued.len());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    if app.report.queued.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  Queue empty — all projects can build freely")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app.report.queued.iter().enumerate().map(|(i, job)| {
        let (prio_color, prio_bg) = priority_colors(job.priority);
        let cmd      = format!("cargo {}", job.args.join(" "));
        let wait_sec = chrono::Utc::now()
            .signed_duration_since(job.queued_at)
            .num_seconds();

        ListItem::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" {} ", job.priority.label()),
                    Style::default().fg(Color::Black).bg(prio_bg).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {}. ", i + 1)),
                Span::styled(
                    truncate(&job.alias, 16),
                    Style::default().fg(prio_color).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("       "),
                Span::styled(truncate(&cmd, 30), Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("  {}s", wait_sec),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
        ])
    }).collect();

    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ "),
        area,
        &mut app.queue_sel,
    );
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let [bindings_area, status_area] = *Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
    ]).split(area) else { return };

    // ── Keybinding bar ────────────────────────────────────────────────────────
    let normal_bindings = vec![
        key_hint("j/k", "navigate"),
        key_hint("Tab", "switch panel"),
        key_hint("+/-", "priority"),
        key_hint("x", "kill"),
        key_hint("c", "cancel"),
        key_hint("X", "kill project"),
        key_hint("s", "slots"),
        key_hint("a", "alias"),
        key_hint("r", "refresh"),
        key_hint("q", "quit"),
    ];

    let input_hint = match app.input_mode {
        InputMode::Normal    => Line::from(normal_bindings),
        InputMode::SlotInput  => Line::from(vec![
            Span::styled("Slots: ", Style::default().fg(Color::Yellow)),
            Span::styled(&app.input_buf, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(Color::White)),
            Span::styled("  Enter=confirm  Esc=cancel", Style::default().fg(Color::DarkGray)),
        ]),
        InputMode::AliasInput => Line::from(vec![
            Span::styled("Alias: ", Style::default().fg(Color::Yellow)),
            Span::styled(&app.input_buf, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(Color::White)),
            Span::styled("  Enter=confirm  Esc=cancel", Style::default().fg(Color::DarkGray)),
        ]),
    };

    frame.render_widget(Paragraph::new(input_hint), bindings_area);

    // ── Status / feedback line ────────────────────────────────────────────────
    let status_color = if app.status_msg.starts_with('✗') {
        Color::Red
    } else if app.status_msg.starts_with('✔') {
        Color::Green
    } else if app.status_msg.starts_with('⚠') {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    frame.render_widget(
        Paragraph::new(app.status_msg.as_str())
            .style(Style::default().fg(status_color))
            .block(Block::default().borders(Borders::TOP))
            .wrap(Wrap { trim: true }),
        status_area,
    );
}

// ── Visual helpers ────────────────────────────────────────────────────────────

fn priority_colors(p: Priority) -> (Color, Color) {
    match p {
        Priority::Critical   => (Color::Black, Color::Red),
        Priority::High       => (Color::Black, Color::LightRed),
        Priority::Normal     => (Color::Black, Color::Blue),
        Priority::Low        => (Color::Black, Color::DarkGray),
        Priority::Background => (Color::DarkGray, Color::Black),
    }
}

fn key_hint<'a>(key: &'a str, desc: &'a str) -> Span<'a> {
    // We return a single span — in practice called inside a Line::from(vec![])
    // Alternate key/desc coloring for readability
    Span::raw(format!("  {}:{}", key, desc))
}

fn format_elapsed(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins  = total_secs / 60;
    let secs  = total_secs % 60;
    format!("{:02}:{:02}", mins, secs)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}
