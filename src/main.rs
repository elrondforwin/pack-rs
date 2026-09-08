mod cache;
mod package;

use std::{
    env, io, process,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use cache::PackageCache;
use package::{Action, PackageList, PackageManager, PackageSource};

const MAIN_MENU: [&str; 5] = [
    "Install Packages",
    "Remove Packages",
    "Update System",
    "Advanced Options",
    "Exit",
];
const REMOVE_MENU: [&str; 5] = [
    "All Installed Packages",
    "Recently Installed Packages",
    "Installed AUR Packages",
    "Orphaned Packages",
    "Back",
];
const ADVANCED_MENU: [&str; 4] = [
    "Clear Helper Cache",
    "Refresh Pack Cache",
    "Refresh Pack Cache (without AUR)",
    "Back",
];

type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> io::Result<()> {
    let route = match parse_route(env::args().skip(1).collect()) {
        Ok(RouteParse::Run(route)) => route,
        Ok(RouteParse::Print(text)) => {
            println!("{text}");
            return Ok(());
        }
        Err(error) => {
            eprintln!("{error}\n\n{}", usage());
            process::exit(2);
        }
    };

    let cache = PackageCache::from_environment()?;
    let manager = PackageManager::detect();
    let mut terminal = TerminalSession::start()?;
    run(&mut terminal, &manager, &cache, route)
}

fn run(
    terminal: &mut TerminalSession,
    manager: &PackageManager,
    cache: &PackageCache,
    route: Route,
) -> io::Result<()> {
    let mut app = App::new();
    match route {
        Route::Tui => {}
        Route::Install => process_request(
            &mut app,
            terminal,
            manager,
            cache,
            Request::Load(PackageSource::Available),
        ),
        Route::Remove => process_request(
            &mut app,
            terminal,
            manager,
            cache,
            Request::Load(PackageSource::Installed),
        ),
        Route::Update => app.show_confirmation(Action::UpdateSystem),
    }

    while !app.should_quit {
        if let Some((source, result)) = app.poll_loading() {
            match result {
                Ok(list) => app.show_packages(source, list),
                Err(error) => app.show_output(source.title(), error),
            }
        }
        terminal
            .terminal_mut()
            .draw(|frame| render(frame, &mut app, manager))?;

        if event::poll(Duration::from_millis(250))? {
            let request = match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => None,
            };
            if let Some(request) = request {
                process_request(&mut app, terminal, manager, cache, request);
            }
        }
    }

    Ok(())
}

fn process_request(
    app: &mut App,
    terminal: &mut TerminalSession,
    manager: &PackageManager,
    cache: &PackageCache,
    request: Request,
) {
    match request {
        Request::Load(source) if source == PackageSource::Available && cache.is_fresh() => {
            match manager.list(cache, source) {
                Ok(list) => app.show_packages(source, list),
                Err(error) => app.show_output(source.title(), error),
            }
        }
        Request::Load(source) => app.start_loading(manager, cache, source),
        Request::Preview { package, installed } => {
            match manager.package_info(&package, installed) {
                Ok(result) => app.show_output(format!("Package Info: {package}"), result.display()),
                Err(error) => app.show_output(format!("Package Info: {package}"), error),
            }
        }
        Request::Execute(action) => match action {
            Action::RefreshCache { without_aur } => match manager.refresh_cache(cache, without_aur)
            {
                Ok(count) => app.show_output(
                    action.title(),
                    format!(
                        "Cache refreshed: {count} packages.\n{}",
                        cache.path().display()
                    ),
                ),
                Err(error) => app.show_output(action.title(), error),
            },
            _ => {
                let title = action.title();
                let result = terminal.with_suspended(|| manager.run_interactive(&action));
                match result {
                    Ok(message) => app.show_output(title, message),
                    Err(error) => app.show_output(title, error),
                }
            }
        },
    }
}

fn render(frame: &mut Frame, app: &mut App, manager: &PackageManager) {
    let cursor = app.cursor;
    match &mut app.screen {
        Screen::Main => render_menu(frame, &MAIN_MENU, cursor),
        Screen::RemoveMenu => render_menu(frame, &REMOVE_MENU, cursor),
        Screen::AdvancedMenu => render_menu(frame, &ADVANCED_MENU, cursor),
        Screen::Packages(view) => render_packages(frame, view, cursor),
        Screen::Confirm(action) => render_confirmation(frame, action, manager),
        Screen::Output(output) => render_output(frame, output),
    }
}

fn render_menu(frame: &mut Frame, items: &[&str], cursor: usize) {
    let area = centered_rect(44, items.len() as u16, frame.area());
    for (index, item) in items.iter().enumerate() {
        let selected = index == cursor;
        let style = if selected {
            Style::default()
                .fg(Color::Indexed(212))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let button_width = item.chars().count() as u16 + 4;
        let button_area = centered_rect(
            button_width,
            1,
            Rect::new(area.x, area.y + index as u16, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(format!("{} {item} ", if selected { ">" } else { " " })).style(style),
            button_area,
        );
    }
}

fn render_packages(frame: &mut Frame, view: &mut PackageView, cursor: usize) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());
    if view.loading {
        frame.render_widget(
            Paragraph::new("Loading packages...")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            areas[0],
        );
        return;
    }
    let selected = view.selected.iter().filter(|selected| **selected).count();
    frame.render_widget(
        Paragraph::new(format!(
            "{} / {} packages - {selected} selected{}",
            view.matches.len(),
            view.packages.len(),
            view.notice
                .as_deref()
                .map_or(String::new(), |notice| format!(" - {notice}"))
        ))
        .style(Style::default().fg(Color::Gray)),
        areas[1],
    );

    render_package_results(frame, areas[0], view, cursor);

    let search = if view.searching {
        format!(
            "/ {}▌{}",
            &view.query[..view.query_cursor],
            &view.query[view.query_cursor..]
        )
    } else if view.query.is_empty() {
        "/ to search".to_owned()
    } else {
        format!("/ {}", view.query)
    };
    frame.render_widget(
        Paragraph::new(search).style(if view.searching {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        }),
        areas[2],
    );
}

fn render_package_results(frame: &mut Frame, area: Rect, view: &mut PackageView, cursor: usize) {
    view.viewport_height = area.height.max(1) as usize;
    if view.matches.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching packages.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        view.scroll_offset = 0;
        return;
    }

    let height = view.viewport_height;
    if cursor < view.scroll_offset {
        view.scroll_offset = cursor;
    } else if cursor >= view.scroll_offset + height {
        view.scroll_offset = cursor + 1 - height;
    }
    view.scroll_offset = view
        .scroll_offset
        .min(view.matches.len().saturating_sub(height));
    let end = (view.scroll_offset + height).min(view.matches.len());

    for (row, index) in view.matches[view.scroll_offset..end].iter().enumerate() {
        let package = &view.packages[*index];
        let cursor_selected = view.scroll_offset + row == cursor;
        let package_selected = view.selected[*index];
        let style = if cursor_selected || package_selected {
            Style::default()
                .fg(Color::Indexed(212))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker_style = if cursor_selected || package_selected {
            style
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut spans = vec![Span::styled(
            format!(
                "{}{} ",
                if cursor_selected { ">" } else { " " },
                if package_selected { "✓" } else { " " }
            ),
            marker_style,
        )];
        if let Some((repository, package_name)) = package.split_once('/') {
            spans.push(Span::styled(
                repository,
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                " › ",
                Style::default().fg(Color::Indexed(240)),
            ));
            spans.push(Span::styled(package_name, style));
        } else {
            spans.push(Span::styled(package.as_str(), style));
        }
        let line = Line::from(spans);
        let y = area.y + area.height - 1 - row as u16;
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(area.x, y, area.width, 1),
        );
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn render_confirmation(frame: &mut Frame, action: &Action, manager: &PackageManager) {
    let command = action
        .command_preview(manager.helper())
        .unwrap_or_else(|error| error);
    let content = format!(
        "{}\n\nCommand to run:\n{command}\n\ny: confirm  n/Esc: cancel",
        action.summary()
    );
    frame.render_widget(
        Paragraph::new(content).wrap(Wrap { trim: true }),
        frame.area(),
    );
}

fn render_output(frame: &mut Frame, output: &OutputView) {
    frame.render_widget(
        Paragraph::new(format!("{}\n\n{}", output.title, output.body))
            .scroll((output.scroll, 0))
            .wrap(Wrap { trim: false }),
        frame.area(),
    );
}

struct App {
    cursor: usize,
    main_cursor: usize,
    remove_cursor: usize,
    advanced_cursor: usize,
    loading: Option<LoadingTask>,
    screen: Screen,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            cursor: 0,
            main_cursor: 0,
            remove_cursor: 0,
            advanced_cursor: 0,
            loading: None,
            screen: Screen::Main,
            should_quit: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Request> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        let mut next_screen = None;
        let mut request = None;
        match &mut self.screen {
            Screen::Main => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Up | KeyCode::Char('k') => previous(&mut self.cursor, MAIN_MENU.len()),
                KeyCode::Down | KeyCode::Char('j') => next(&mut self.cursor, MAIN_MENU.len()),
                KeyCode::Enter => match self.cursor {
                    0 => request = Some(Request::Load(PackageSource::Available)),
                    1 => next_screen = Some(Screen::RemoveMenu),
                    2 => next_screen = Some(Screen::Confirm(Action::UpdateSystem)),
                    3 => next_screen = Some(Screen::AdvancedMenu),
                    _ => self.should_quit = true,
                },
                _ => {}
            },
            Screen::RemoveMenu => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc => next_screen = Some(Screen::Main),
                KeyCode::Up | KeyCode::Char('k') => previous(&mut self.cursor, REMOVE_MENU.len()),
                KeyCode::Down | KeyCode::Char('j') => next(&mut self.cursor, REMOVE_MENU.len()),
                KeyCode::Enter => match self.cursor {
                    0 => request = Some(Request::Load(PackageSource::Installed)),
                    1 => request = Some(Request::Load(PackageSource::Recent)),
                    2 => request = Some(Request::Load(PackageSource::Aur)),
                    3 => request = Some(Request::Load(PackageSource::Orphaned)),
                    _ => next_screen = Some(Screen::Main),
                },
                _ => {}
            },
            Screen::AdvancedMenu => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc => next_screen = Some(Screen::Main),
                KeyCode::Up | KeyCode::Char('k') => previous(&mut self.cursor, ADVANCED_MENU.len()),
                KeyCode::Down | KeyCode::Char('j') => next(&mut self.cursor, ADVANCED_MENU.len()),
                KeyCode::Enter => match self.cursor {
                    0 => next_screen = Some(Screen::Confirm(Action::ClearHelperCache)),
                    1 => {
                        next_screen =
                            Some(Screen::Confirm(Action::RefreshCache { without_aur: false }))
                    }
                    2 => {
                        next_screen =
                            Some(Screen::Confirm(Action::RefreshCache { without_aur: true }))
                    }
                    _ => next_screen = Some(Screen::Main),
                },
                _ => {}
            },
            Screen::Packages(view) if view.loading => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Esc => {
                    self.loading = None;
                    next_screen = Some(match view.back {
                        PackageBack::Main => Screen::Main,
                        PackageBack::RemoveMenu => Screen::RemoveMenu,
                    });
                }
                _ => {}
            },
            Screen::Packages(view) => match key.code {
                KeyCode::Char('/') if !view.searching => view.searching = true,
                KeyCode::Esc => {
                    next_screen = Some(match view.back {
                        PackageBack::Main => Screen::Main,
                        PackageBack::RemoveMenu => Screen::RemoveMenu,
                    })
                }
                KeyCode::Char('a')
                    if view.searching && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    view.query_cursor = 0;
                }
                KeyCode::Char('e')
                    if view.searching && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    view.query_cursor = view.query.len();
                }
                KeyCode::Char('h') | KeyCode::Char('w')
                    if view.searching && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    delete_previous_word(view);
                    view.refresh_matches();
                    self.cursor = 0;
                }
                KeyCode::Backspace
                    if view.searching && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    delete_previous_word(view);
                    view.refresh_matches();
                    self.cursor = 0;
                }
                KeyCode::Backspace if view.searching => {
                    delete_previous_character(view);
                    view.refresh_matches();
                    self.cursor = 0;
                }
                KeyCode::Home if view.searching => view.query_cursor = 0,
                KeyCode::End if view.searching => view.query_cursor = view.query.len(),
                KeyCode::Left
                    if view.searching && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    move_query_word_left(view);
                }
                KeyCode::Right
                    if view.searching && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    move_query_word_right(view);
                }
                KeyCode::Left if view.searching => move_query_left(view),
                KeyCode::Right if view.searching => move_query_right(view),
                KeyCode::Char(character) if view.searching && !character.is_control() => {
                    view.query.insert(view.query_cursor, character);
                    view.query_cursor += character.len_utf8();
                    view.refresh_matches();
                    self.cursor = 0;
                }
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Up | KeyCode::Char('k') => scroll_up(&mut self.cursor, view.matches.len()),
                KeyCode::Down | KeyCode::Char('j') => {
                    scroll_down(&mut self.cursor, view.matches.len())
                }
                KeyCode::PageUp => {
                    let length = view.matches.len();
                    self.cursor = self
                        .cursor
                        .saturating_add(PAGE_SIZE)
                        .min(length.saturating_sub(1));
                }
                KeyCode::PageDown => {
                    self.cursor = self.cursor.saturating_sub(PAGE_SIZE);
                }
                KeyCode::Char(' ') | KeyCode::Tab => {
                    if let Some(index) = view.matches.get(self.cursor)
                        && let Some(selected) = view.selected.get_mut(*index)
                    {
                        *selected = !*selected;
                        scroll_down(&mut self.cursor, view.matches.len());
                    }
                }
                KeyCode::Char('a') => {
                    let select_all = view.matches.iter().any(|index| !view.selected[*index]);
                    for &index in &view.matches {
                        view.selected[index] = select_all;
                    }
                }
                KeyCode::Char('p') => {
                    if let Some(index) = view.matches.get(self.cursor)
                        && let Some(package) = view.packages.get(*index)
                    {
                        request = Some(Request::Preview {
                            package: package.clone(),
                            installed: view.source.is_installed(),
                        });
                    }
                }
                KeyCode::Enter => {
                    let mut packages = selected_packages(view);
                    if packages.is_empty()
                        && let Some(index) = view.matches.get(self.cursor)
                        && let Some(package) = view.packages.get(*index)
                    {
                        packages.push(package.clone());
                    }
                    if packages.is_empty() {
                        view.notice = None;
                    } else {
                        let action = if view.source.is_installed() {
                            Action::Remove(packages)
                        } else {
                            Action::Install(packages)
                        };
                        if view.source.is_installed() {
                            next_screen = Some(Screen::Confirm(action));
                        } else {
                            request = Some(Request::Execute(action));
                        }
                    }
                }
                _ => {}
            },
            Screen::Confirm(action) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    request = Some(Request::Execute(action.clone()))
                }
                KeyCode::Char('n') | KeyCode::Esc => next_screen = Some(Screen::Main),
                KeyCode::Char('q') => self.should_quit = true,
                _ => {}
            },
            Screen::Output(output) => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Enter | KeyCode::Esc => next_screen = Some(Screen::Main),
                KeyCode::Up | KeyCode::Char('k') => output.scroll = output.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    output.scroll = output.scroll.saturating_add(1)
                }
                _ => {}
            },
        }

        if let Some(screen) = next_screen {
            self.set_screen(screen);
        }
        request
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<Request> {
        match (&mut self.screen, mouse.kind) {
            (Screen::Packages(view), MouseEventKind::ScrollUp) => {
                scroll_up(&mut self.cursor, view.matches.len());
            }
            (Screen::Packages(view), MouseEventKind::ScrollDown) => {
                scroll_down(&mut self.cursor, view.matches.len());
            }
            (Screen::Output(output), MouseEventKind::ScrollUp) => {
                output.scroll = output.scroll.saturating_sub(1);
            }
            (Screen::Output(output), MouseEventKind::ScrollDown) => {
                output.scroll = output.scroll.saturating_add(1);
            }
            _ => {}
        }
        None
    }

    fn set_screen(&mut self, screen: Screen) {
        match &self.screen {
            Screen::Main => self.main_cursor = self.cursor,
            Screen::RemoveMenu => self.remove_cursor = self.cursor,
            Screen::AdvancedMenu => self.advanced_cursor = self.cursor,
            _ => {}
        }
        self.cursor = match &screen {
            Screen::Main => self.main_cursor,
            Screen::RemoveMenu => self.remove_cursor,
            Screen::AdvancedMenu => self.advanced_cursor,
            _ => 0,
        };
        self.screen = screen;
    }

    fn start_loading(
        &mut self,
        manager: &PackageManager,
        cache: &PackageCache,
        source: PackageSource,
    ) {
        let (sender, receiver) = mpsc::channel();
        let manager = manager.clone();
        let cache = cache.clone();
        thread::spawn(move || {
            let _ = sender.send(manager.list(&cache, source));
        });
        self.loading = Some(LoadingTask { source, receiver });
        let back = match source {
            PackageSource::Available => PackageBack::Main,
            PackageSource::Installed
            | PackageSource::Recent
            | PackageSource::Aur
            | PackageSource::Orphaned => PackageBack::RemoveMenu,
        };
        self.set_screen(Screen::Packages(PackageView {
            source,
            packages: Vec::new(),
            matches: Vec::new(),
            back,
            selected: Vec::new(),
            notice: None,
            query: String::new(),
            query_cursor: 0,
            searching: false,
            loading: true,
            scroll_offset: 0,
            viewport_height: 1,
        }));
    }

    fn poll_loading(&mut self) -> Option<(PackageSource, Result<PackageList, String>)> {
        let task = self.loading.as_ref()?;
        match task.receiver.try_recv() {
            Ok(result) => {
                let source = task.source;
                self.loading = None;
                Some((source, result))
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                let source = task.source;
                self.loading = None;
                Some((source, Err("Package loading failed.".to_owned())))
            }
        }
    }

    fn show_packages(&mut self, source: PackageSource, list: PackageList) {
        if list.packages.is_empty() {
            self.show_output(
                source.title(),
                list.notice
                    .unwrap_or_else(|| "No packages found.".to_owned()),
            );
            return;
        }
        let packages = list.packages;
        let matches = fuzzy_matches("", &packages);
        let back = match source {
            PackageSource::Available => PackageBack::Main,
            PackageSource::Installed
            | PackageSource::Recent
            | PackageSource::Aur
            | PackageSource::Orphaned => PackageBack::RemoveMenu,
        };
        self.set_screen(Screen::Packages(PackageView {
            selected: vec![false; packages.len()],
            packages,
            matches,
            back,
            source,
            notice: list.notice,
            query: String::new(),
            query_cursor: 0,
            searching: true,
            loading: false,
            scroll_offset: 0,
            viewport_height: 1,
        }));
    }

    fn show_confirmation(&mut self, action: Action) {
        self.set_screen(Screen::Confirm(action));
    }

    fn show_output(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.set_screen(Screen::Output(OutputView {
            title: title.into(),
            body: body.into(),
            scroll: 0,
        }));
    }
}

enum Screen {
    Main,
    RemoveMenu,
    AdvancedMenu,
    Packages(PackageView),
    Confirm(Action),
    Output(OutputView),
}

struct LoadingTask {
    source: PackageSource,
    receiver: Receiver<Result<PackageList, String>>,
}

struct PackageView {
    source: PackageSource,
    packages: Vec<String>,
    matches: Vec<usize>,
    back: PackageBack,
    selected: Vec<bool>,
    notice: Option<String>,
    query: String,
    query_cursor: usize,
    searching: bool,
    loading: bool,
    scroll_offset: usize,
    viewport_height: usize,
}

#[derive(Clone, Copy)]
enum PackageBack {
    Main,
    RemoveMenu,
}

impl PackageView {
    fn refresh_matches(&mut self) {
        self.matches = fuzzy_matches(&self.query, &self.packages);
    }
}

fn delete_previous_character(view: &mut PackageView) {
    if view.query_cursor == 0 {
        return;
    }
    let start = view.query[..view.query_cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index);
    view.query.drain(start..view.query_cursor);
    view.query_cursor = start;
}

fn delete_previous_word(view: &mut PackageView) {
    let end = view.query_cursor;
    let mut start = end;
    while start > 0 {
        let (index, character) = view.query[..start].char_indices().next_back().unwrap();
        if !character.is_whitespace() {
            start = index;
            break;
        }
        start = index;
    }
    while start > 0 {
        let (index, character) = view.query[..start].char_indices().next_back().unwrap();
        if character.is_whitespace() {
            break;
        }
        start = index;
    }
    view.query.drain(start..end);
    view.query_cursor = start;
}

fn move_query_left(view: &mut PackageView) {
    if view.query_cursor > 0 {
        view.query_cursor = view.query[..view.query_cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
    }
}

fn move_query_right(view: &mut PackageView) {
    if view.query_cursor < view.query.len() {
        view.query_cursor += view.query[view.query_cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    }
}

fn move_query_word_left(view: &mut PackageView) {
    let mut cursor = view.query_cursor;
    while cursor > 0 {
        let (index, character) = view.query[..cursor].char_indices().next_back().unwrap();
        cursor = index;
        if !character.is_whitespace() {
            break;
        }
    }
    while cursor > 0 {
        let (index, character) = view.query[..cursor].char_indices().next_back().unwrap();
        if character.is_whitespace() {
            break;
        }
        cursor = index;
    }
    view.query_cursor = cursor;
}

fn move_query_word_right(view: &mut PackageView) {
    let mut cursor = view.query_cursor;
    while cursor < view.query.len() {
        let character = view.query[cursor..].chars().next().unwrap();
        cursor += character.len_utf8();
        if !character.is_whitespace() {
            break;
        }
    }
    while cursor < view.query.len() {
        let character = view.query[cursor..].chars().next().unwrap();
        if character.is_whitespace() {
            break;
        }
        cursor += character.len_utf8();
    }
    view.query_cursor = cursor;
}

struct OutputView {
    title: String,
    body: String,
    scroll: u16,
}

enum Request {
    Load(PackageSource),
    Preview { package: String, installed: bool },
    Execute(Action),
}

#[derive(Clone, Copy)]
enum Route {
    Tui,
    Install,
    Remove,
    Update,
}

enum RouteParse {
    Run(Route),
    Print(&'static str),
}

fn parse_route(arguments: Vec<String>) -> Result<RouteParse, String> {
    match arguments.as_slice() {
        [] => Ok(RouteParse::Run(Route::Tui)),
        [argument] if matches!(argument.as_str(), "-h" | "--help") => {
            Ok(RouteParse::Print(usage()))
        }
        [argument] if matches!(argument.as_str(), "-V" | "--version") => {
            Ok(RouteParse::Print("pack 0.1.0"))
        }
        [argument] if argument == "install" => Ok(RouteParse::Run(Route::Install)),
        [argument] if matches!(argument.as_str(), "remove" | "uninstall") => {
            Ok(RouteParse::Run(Route::Remove))
        }
        [argument] if argument == "update" => Ok(RouteParse::Run(Route::Update)),
        _ => Err("Unknown or incomplete command-line argument.".to_owned()),
    }
}

fn usage() -> &'static str {
    "Usage: pack [install|remove|uninstall|update]\n\nLaunches the interactive interface when no command is provided."
}

fn selected_packages(view: &PackageView) -> Vec<String> {
    view.packages
        .iter()
        .zip(&view.selected)
        .filter(|(_, selected)| **selected)
        .map(|(package, _)| package.clone())
        .collect()
}

fn fuzzy_matches(query: &str, packages: &[String]) -> Vec<usize> {
    if query.is_empty() {
        let mut matches = Vec::with_capacity(packages.len());
        matches.extend(
            packages
                .iter()
                .enumerate()
                .filter(|(_, package)| !is_aur_package(package))
                .map(|(index, _)| index),
        );
        matches.extend(
            packages
                .iter()
                .enumerate()
                .filter(|(_, package)| is_aur_package(package))
                .map(|(index, _)| index),
        );
        return matches;
    }

    let query = query.to_lowercase();
    let mut matches = packages
        .iter()
        .enumerate()
        .filter_map(|(index, package)| fuzzy_score(&query, package).map(|score| (index, score)))
        .collect::<Vec<_>>();
    matches.sort_unstable_by(|(left_index, left_score), (right_index, right_score)| {
        is_aur_package(&packages[*left_index])
            .cmp(&is_aur_package(&packages[*right_index]))
            .then_with(|| {
                is_exact_package_name(&query, &packages[*right_index])
                    .cmp(&is_exact_package_name(&query, &packages[*left_index]))
            })
            .then_with(|| right_score.cmp(left_score))
            .then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn is_aur_package(candidate: &str) -> bool {
    candidate
        .split_once('/')
        .is_some_and(|(repository, _)| repository.eq_ignore_ascii_case("aur"))
}

fn is_exact_package_name(query: &str, candidate: &str) -> bool {
    !query.is_empty()
        && candidate
            .rsplit_once('/')
            .map_or(candidate, |(_, package_name)| package_name)
            .eq_ignore_ascii_case(query)
}

fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    // ponytail: simple sequential scorer; replace only if ranking quality becomes a measurable issue.
    let candidate = candidate.to_lowercase();
    let characters = candidate.chars().collect::<Vec<_>>();
    let mut search_from = 0;
    let mut previous = None;
    let mut score = 0;

    for query_character in query.chars() {
        let position = characters[search_from..]
            .iter()
            .position(|character| *character == query_character)
            .map(|offset| search_from + offset)?;
        score += 10;
        if previous.is_some_and(|last| last + 1 == position) {
            score += 15;
        }
        if position == 0 || matches!(characters[position - 1], '/' | '-' | '_' | '.') {
            score += 10;
        }
        score -= position as i32;
        previous = Some(position);
        search_from = position + 1;
    }

    Some(score)
}

fn previous(cursor: &mut usize, length: usize) {
    if length > 0 {
        *cursor = if *cursor == 0 {
            length - 1
        } else {
            *cursor - 1
        };
    }
}

fn next(cursor: &mut usize, length: usize) {
    if length > 0 {
        *cursor = (*cursor + 1) % length;
    }
}

const PAGE_SIZE: usize = 10;

fn scroll_up(cursor: &mut usize, length: usize) {
    if length > 0 {
        *cursor = (*cursor).saturating_add(1).min(length - 1);
    }
}

fn scroll_down(cursor: &mut usize, length: usize) {
    if length > 0 {
        *cursor = (*cursor).saturating_sub(1).min(length - 1);
    }
}

struct TerminalSession {
    terminal: AppTerminal,
    active: bool,
}

impl TerminalSession {
    fn start() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.hide_cursor() {
                    let _ = disable_raw_mode();
                    let _ = execute!(
                        terminal.backend_mut(),
                        DisableMouseCapture,
                        LeaveAlternateScreen
                    );
                    return Err(error);
                }
                Ok(Self {
                    terminal,
                    active: true,
                })
            }
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
                Err(error)
            }
        }
    }

    fn terminal_mut(&mut self) -> &mut AppTerminal {
        &mut self.terminal
    }

    fn with_suspended<T>(
        &mut self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.suspend()
            .map_err(|error| format!("Terminal kapatılamadı: {error}"))?;
        let operation_result = operation();
        let resume_result = self
            .resume()
            .map_err(|error| format!("Terminal açılamadı: {error}"));
        match (operation_result, resume_result) {
            (_, Err(error)) => Err(error),
            (result, Ok(())) => result,
        }
    }

    fn suspend(&mut self) -> io::Result<()> {
        if self.active {
            disable_raw_mode()?;
            if let Err(error) = execute!(
                self.terminal.backend_mut(),
                DisableMouseCapture,
                LeaveAlternateScreen
            ) {
                let _ = enable_raw_mode();
                return Err(error);
            }
            self.active = false;
            self.terminal.show_cursor()?;
        }
        Ok(())
    }

    fn resume(&mut self) -> io::Result<()> {
        if !self.active {
            enable_raw_mode()?;
            if let Err(error) = execute!(
                self.terminal.backend_mut(),
                EnterAlternateScreen,
                EnableMouseCapture
            ) {
                let _ = disable_raw_mode();
                return Err(error);
            }
            if let Err(error) = self
                .terminal
                .clear()
                .and_then(|_| self.terminal.hide_cursor())
            {
                let _ = disable_raw_mode();
                let _ = execute!(
                    self.terminal.backend_mut(),
                    DisableMouseCapture,
                    LeaveAlternateScreen
                );
                return Err(error);
            }
            self.active = true;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(
                self.terminal.backend_mut(),
                DisableMouseCapture,
                LeaveAlternateScreen
            );
            let _ = self.terminal.show_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn menu_cursor_wraps_at_both_ends() {
        let mut cursor = 0;
        previous(&mut cursor, MAIN_MENU.len());
        assert_eq!(cursor, MAIN_MENU.len() - 1);
        next(&mut cursor, MAIN_MENU.len());
        assert_eq!(cursor, 0);
    }

    #[test]
    fn selected_packages_only_returns_checked_entries() {
        let view = PackageView {
            source: PackageSource::Available,
            packages: vec!["core/bash".to_owned(), "extra/fzf".to_owned()],
            matches: vec![0, 1],
            back: PackageBack::Main,
            selected: vec![true, false],
            notice: None,
            query: String::new(),
            query_cursor: 0,
            searching: false,
            loading: false,
            scroll_offset: 0,
            viewport_height: 1,
        };
        assert_eq!(selected_packages(&view), vec!["core/bash"]);
    }

    #[test]
    fn escape_returns_to_the_menu_that_opened_package_list() {
        let mut app = App {
            cursor: 0,
            main_cursor: 0,
            remove_cursor: 1,
            advanced_cursor: 0,
            loading: None,
            screen: Screen::Packages(PackageView {
                source: PackageSource::Installed,
                packages: vec!["extra/fzf".to_owned()],
                matches: vec![0],
                back: PackageBack::RemoveMenu,
                selected: vec![false],
                notice: None,
                query: String::new(),
                query_cursor: 0,
                searching: true,
                loading: false,
                scroll_offset: 0,
                viewport_height: 1,
            }),
            should_quit: false,
        };

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(matches!(app.screen, Screen::RemoveMenu));
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn cli_routes_match_reference_commands() {
        assert!(matches!(
            parse_route(vec!["install".to_owned()]),
            Ok(RouteParse::Run(Route::Install))
        ));
        assert!(matches!(
            parse_route(vec!["uninstall".to_owned()]),
            Ok(RouteParse::Run(Route::Remove))
        ));
        assert!(matches!(
            parse_route(vec!["update".to_owned()]),
            Ok(RouteParse::Run(Route::Update))
        ));
    }

    #[test]
    fn fuzzy_search_filters_case_insensitively_and_prefers_contiguous_matches() {
        let packages = vec![
            "core/bash".to_owned(),
            "extra/base-devel".to_owned(),
            "extra/BASH-completion".to_owned(),
        ];

        assert_eq!(fuzzy_matches("bsh", &packages), vec![0, 2]);
    }

    #[test]
    fn exact_package_name_beats_fuzzy_matches() {
        let packages = vec![
            "extra/discord-account-manager-git".to_owned(),
            "extra/discord".to_owned(),
        ];

        assert_eq!(fuzzy_matches("discord", &packages), vec![1, 0]);
    }

    #[test]
    fn aur_packages_always_follow_non_aur_packages() {
        let packages = vec!["aur/discord".to_owned(), "extra/discord-helper".to_owned()];

        assert_eq!(fuzzy_matches("discord", &packages), vec![1, 0]);
    }

    #[test]
    fn search_input_keeps_selection_on_the_original_package() {
        let mut app = App {
            cursor: 0,
            main_cursor: 0,
            remove_cursor: 0,
            advanced_cursor: 0,
            loading: None,
            screen: Screen::Packages(PackageView {
                source: PackageSource::Available,
                packages: vec!["core/bash".to_owned(), "extra/fzf".to_owned()],
                matches: vec![0, 1],
                back: PackageBack::Main,
                selected: vec![true, false],
                notice: None,
                query: String::new(),
                query_cursor: 0,
                searching: false,
                loading: false,
                scroll_offset: 0,
                viewport_height: 1,
            }),
            should_quit: false,
        };

        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

        let Screen::Packages(view) = app.screen else {
            panic!("package screen should be preserved");
        };
        assert_eq!(view.query, "fz");
        assert_eq!(fuzzy_matches(&view.query, &view.packages), vec![1]);
        assert_eq!(selected_packages(&view), vec!["core/bash"]);
    }

    #[test]
    fn space_selects_current_package_and_moves_to_next_match() {
        let mut app = App {
            cursor: 1,
            main_cursor: 0,
            remove_cursor: 0,
            advanced_cursor: 0,
            loading: None,
            screen: Screen::Packages(PackageView {
                source: PackageSource::Available,
                packages: vec!["core/bash".to_owned(), "extra/fzf".to_owned()],
                matches: vec![0, 1],
                back: PackageBack::Main,
                selected: vec![false, false],
                notice: None,
                query: String::new(),
                query_cursor: 0,
                searching: false,
                loading: false,
                scroll_offset: 0,
                viewport_height: 1,
            }),
            should_quit: false,
        };

        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        let Screen::Packages(view) = app.screen else {
            panic!("package screen should be preserved");
        };
        assert_eq!(app.cursor, 0);
        assert_eq!(view.selected, vec![false, true]);
    }

    #[test]
    fn mouse_scroll_stays_within_the_package_list() {
        let mut cursor = 0;
        scroll_up(&mut cursor, 3);
        assert_eq!(cursor, 1);

        cursor = 2;
        scroll_down(&mut cursor, 3);
        assert_eq!(cursor, 1);

        scroll_down(&mut cursor, 3);
        scroll_down(&mut cursor, 3);
        assert_eq!(cursor, 0);
    }
}
