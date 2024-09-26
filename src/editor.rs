use crate::config::{key_to_string, Config};
use crate::error::{OxError, Result};
use crate::ui::{size, Feedback, Terminal, HELP_TEXT};
use crossterm::{
    event::{read, Event as CEvent, KeyCode as KCode, KeyModifiers as KMod},
    style::{Attribute, SetAttribute, SetBackgroundColor as Bg, SetForegroundColor as Fg},
    terminal::{Clear, ClearType as ClType},
};
use kaolinite::event::{Error as KError, Event, Status};
use kaolinite::utils::{Loc, Size};
use kaolinite::Document;
use mlua::Lua;
use std::io::{ErrorKind, Write};
use std::time::Instant;
use synoptic::{trim, Highlighter, TokOpt};

mod mouse;

/// For managing all editing and rendering of cactus
pub struct Editor {
    /// Interface for writing to the terminal
    pub terminal: Terminal,
    /// Whether to rerender the editor on the next cycle
    pub needs_rerender: bool,
    /// Configuration information for the editor
    pub config: Config,
    /// Storage of all the documents opened in the editor
    pub doc: Vec<Document>,
    /// Syntax highlighting integration
    pub highlighter: Vec<Highlighter>,
    /// Pointer to the document that is currently being edited
    pub ptr: usize,
    /// true if the editor is still running, false otherwise
    pub active: bool,
    /// true if the editor should show a greeting message on next render
    pub greet: bool,
    /// Whether or not to show the help message
    pub help: bool,
    /// The feedback message to display below the status line
    pub feedback: Feedback,
    /// Will be some if there is an outstanding command to be run
    pub command: Option<String>,
    /// Will store the last time the editor was interacted with (to track inactivity)
    pub last_active: Instant,
    /// Used for storing amount to push document down
    push_down: usize,
    /// Used to cache the location of the configuration file
    pub config_path: String,
    /// This is a handy place to figure out if the user is currently pasting something or not
    pub paste_flag: bool,
}

impl Editor {
    /// Create a new instance of the editor
    pub fn new(lua: &Lua) -> Result<Self> {
        let config = Config::new(lua)?;
        Ok(Self {
            doc: vec![],
            ptr: 0,
            terminal: Terminal::new(config.terminal.clone()),
            config,
            active: true,
            greet: false,
            help: false,
            needs_rerender: true,
            highlighter: vec![],
            feedback: Feedback::None,
            command: None,
            last_active: Instant::now(),
            push_down: 1,
            config_path: "~/.oxrc".to_string(),
            paste_flag: false,
        })
    }

    /// Load the configuration values
    pub fn load_config(&mut self, path: String, lua: &Lua) -> Result<()> {
        self.config_path = path.clone();
        let result = self.config.read(path, lua);
        // Display any warnings if the user configuration couldn't be found
        if let Err(OxError::Config(msg)) = result {
            if msg == "Not Found" {
                let warn = "No configuration file found, using default configuration".to_string();
                self.feedback = Feedback::Warning(warn);
            }
        } else {
            result?
        };
        // Calculate the correct push down based on config
        self.push_down = if self.config.tab_line.borrow().enabled {
            1
        } else {
            0
        };
        Ok(())
    }

    /// Function to create a new document
    pub fn blank(&mut self) -> Result<()> {
        let mut size = size()?;
        size.h = size.h.saturating_sub(1 + self.push_down);
        let mut doc = Document::new(size);
        doc.set_tab_width(self.config.document.borrow().tab_width);
        // Load all the lines within viewport into the document
        doc.load_to(size.h);
        // Update in the syntax highlighter
        let mut highlighter = Highlighter::new(4);
        highlighter.run(&doc.lines);
        self.highlighter.push(highlighter);
        // Add document to documents
        self.doc.push(doc);
        Ok(())
    }

    /// Create a new document and move to it
    pub fn new_document(&mut self) -> Result<()> {
        self.blank()?;
        self.ptr = self.doc.len().saturating_sub(1);
        Ok(())
    }

    /// Function to open a document into the editor
    pub fn open(&mut self, file_name: String) -> Result<()> {
        let mut size = size()?;
        size.h = size.h.saturating_sub(1 + self.push_down);
        let mut doc = Document::open(size, file_name.clone())?;
        doc.set_tab_width(self.config.document.borrow().tab_width);
        // Load all the lines within viewport into the document
        doc.load_to(size.h);
        // Update in the syntax highlighter
        let mut ext = file_name.split('.').last().unwrap_or("");
        if ext == "oxrc" {
            ext = "lua"
        }
        let mut highlighter = self
            .config
            .syntax_highlighting
            .borrow()
            .get_highlighter(&ext);
        highlighter.run(&doc.lines);
        self.highlighter.push(highlighter);
        doc.undo_mgmt.saved();
        // Add document to documents
        self.doc.push(doc);
        Ok(())
    }

    /// Function to ask the user for a file to open
    pub fn open_document(&mut self) -> Result<()> {
        let path = self.prompt("File to open")?;
        self.open(path)?;
        self.ptr = self.doc.len().saturating_sub(1);
        Ok(())
    }

    /// Function to try opening a document, and if it doesn't exist, create it
    pub fn open_or_new(&mut self, file_name: String) -> Result<()> {
        let file = self.open(file_name.clone());
        if let Err(OxError::Kaolinite(KError::Io(ref os))) = file {
            if os.kind() == ErrorKind::NotFound {
                self.blank()?;
                let binding = file_name.clone();
                let ext = binding.split('.').last().unwrap_or("");
                self.doc.last_mut().unwrap().file_name = Some(file_name);
                self.doc.last_mut().unwrap().modified = true;
                let highlighter = self
                    .config
                    .syntax_highlighting
                    .borrow()
                    .get_highlighter(&ext);
                *self.highlighter.last_mut().unwrap() = highlighter;
                self.highlighter
                    .last_mut()
                    .unwrap()
                    .run(&self.doc.last_mut().unwrap().lines);
                Ok(())
            } else {
                file
            }
        } else {
            file
        }
    }

    /// Returns a document at a certain index
    pub fn get_doc(&mut self, idx: usize) -> &mut Document {
        self.doc.get_mut(idx).unwrap()
    }

    /// Gets a reference to the current document
    pub fn doc(&self) -> &Document {
        self.doc.get(self.ptr).unwrap()
    }

    /// Gets a mutable reference to the current document
    pub fn doc_mut(&mut self) -> &mut Document {
        self.doc.get_mut(self.ptr).unwrap()
    }

    /// Gets the number of documents currently open
    pub fn doc_len(&mut self) -> usize {
        self.doc.len()
    }

    /// Returns a highlighter at a certain index
    pub fn get_highlighter(&mut self, idx: usize) -> &mut Highlighter {
        self.highlighter.get_mut(idx).unwrap()
    }

    /// Gets a mutable reference to the current document
    pub fn highlighter(&mut self) -> &mut Highlighter {
        self.highlighter.get_mut(self.ptr).unwrap()
    }

    /// Execute an edit event
    pub fn exe(&mut self, ev: Event) -> Result<()> {
        self.doc_mut().exe(ev)?;
        // TODO: Check for change in event type and commit to undo/redo stack if present
        Ok(())
    }

    /// Initialise the editor
    pub fn init(&mut self) -> Result<()> {
        self.terminal.start()?;
        Ok(())
    }

    /// Create a blank document if none are already opened
    pub fn new_if_empty(&mut self) -> Result<()> {
        // If no documents were provided, create a new empty document
        if self.doc.is_empty() {
            self.blank()?;
            self.greet = true && self.config.greeting_message.borrow().enabled;
        }
        Ok(())
    }

    /// Complete one cycle of the editor
    /// This function will return a key press code if applicable
    pub fn cycle(&mut self, lua: &Lua) -> Result<Option<String>> {
        // Run the editor
        self.render(&lua)?;
        // Wait for an event
        let event = read()?;
        self.needs_rerender = match event {
            CEvent::Mouse(event) => match event.kind {
                crossterm::event::MouseEventKind::Moved => false,
                _ => true,
            },
            _ => true,
        };
        match event {
            CEvent::Key(key) => {
                // Check period of inactivity
                let end = Instant::now();
                let inactivity = end.duration_since(self.last_active).as_millis() as usize;
                if inactivity > self.config.document.borrow().undo_period * 1000 {
                    self.doc_mut().commit();
                }
                // Predict whether the user is currently pasting text (based on rapid activity)
                self.paste_flag = inactivity < 5;
                // Register this activity
                self.last_active = Instant::now();
                // Editing - these key bindings can't be modified (only added to)!
                match (key.modifiers, key.code) {
                    // Core key bindings (non-configurable behaviour)
                    (KMod::SHIFT | KMod::NONE, KCode::Char(ch)) => self.character(ch)?,
                    (KMod::NONE, KCode::Tab) => self.character('\t')?,
                    (KMod::NONE, KCode::Backspace) => self.backspace()?,
                    (KMod::NONE, KCode::Delete) => self.delete()?,
                    (KMod::NONE, KCode::Enter) => self.enter()?,
                    _ => (),
                }
                // Check user-defined key combinations (includes defaults if not modified)
                return Ok(Some(key_to_string(key.modifiers, key.code)));
            }
            CEvent::Resize(w, h) => {
                // Ensure all lines in viewport are loaded
                let max = self.dent();
                self.doc_mut().size.w = w.saturating_sub(max as u16) as usize;
                self.doc_mut().size.h = h.saturating_sub(3) as usize;
                let max = self.doc().offset.x + self.doc().size.h;
                self.doc_mut().load_to(max + 1);
            }
            CEvent::Mouse(mouse_event) => {
                self.handle_mouse_event(mouse_event);
                return Ok(None);
            }
            _ => (),
        }
        self.feedback = Feedback::None;
        Ok(None)
    }

    /// Append any missed lines to the syntax highlighter
    pub fn update_highlighter(&mut self) -> Result<()> {
        if self.active {
            let actual = self
                .doc
                .get(self.ptr)
                .and_then(|d| Some(d.loaded_to))
                .unwrap_or(0);
            let percieved = self.highlighter().line_ref.len();
            if percieved < actual {
                let diff = actual.saturating_sub(percieved);
                for i in 0..diff {
                    let line = &self.doc[self.ptr].lines[percieved + i];
                    self.highlighter[self.ptr].append(line);
                }
            }
        }
        Ok(())
    }

    /// Render a single frame of the editor in it's current state
    pub fn render(&mut self, lua: &Lua) -> Result<()> {
        if !self.needs_rerender {
            return Ok(());
        }
        self.needs_rerender = false;
        self.terminal.hide_cursor()?;
        let Size { w, mut h } = size()?;
        h = h.saturating_sub(1 + self.push_down);
        // Update the width of the document in case of update
        let max = self.dent();
        self.doc_mut().size.w = w.saturating_sub(max) as usize;
        // Render the tab line
        let tab_enabled = self.config.tab_line.borrow().enabled;
        if tab_enabled {
            self.render_tab_line(w)?;
        }
        // Run through each line of the terminal, rendering the correct line
        self.render_document(w, h)?;
        // Leave last line for status line
        self.render_status_line(&lua, w, h)?;
        // Render greeting or help message if applicable
        if self.greet {
            self.render_greeting(w, h)?;
        } else if self.help {
            self.render_help_message(w, h)?;
        }
        // Render feedback line
        self.render_feedback_line(w, h)?;
        // Move cursor to the correct location and perform render
        if let Some(Loc { x, y }) = self.doc().cursor_loc_in_screen() {
            self.terminal.show_cursor()?;
            self.terminal.goto(x + max, y + self.push_down)?;
        }
        self.terminal.flush()?;
        Ok(())
    }

    /// Render the lines of the document
    fn render_document(&mut self, _w: usize, h: usize) -> Result<()> {
        for y in 0..(h as u16) {
            self.terminal
                .goto(0, y as usize + self.push_down as usize)?;
            // Start background colour
            write!(
                self.terminal.stdout,
                "{}",
                Bg(self.config.colors.borrow().editor_bg.to_color()?)
            )?;
            write!(
                self.terminal.stdout,
                "{}",
                Fg(self.config.colors.borrow().editor_fg.to_color()?)
            )?;
            // Write line number of document
            if self.config.line_numbers.borrow().enabled {
                let num = self.doc().line_number(y as usize + self.doc().offset.y);
                let padding_left = " ".repeat(self.config.line_numbers.borrow().padding_left);
                let padding_right = " ".repeat(self.config.line_numbers.borrow().padding_right);
                write!(
                    self.terminal.stdout,
                    "{}{}{}{}{}│{}{}",
                    Bg(self.config.colors.borrow().line_number_bg.to_color()?),
                    Fg(self.config.colors.borrow().line_number_fg.to_color()?),
                    padding_left,
                    num,
                    padding_right,
                    Fg(self.config.colors.borrow().editor_fg.to_color()?),
                    Bg(self.config.colors.borrow().editor_bg.to_color()?),
                )?;
            }
            write!(self.terminal.stdout, "{}", Clear(ClType::UntilNewLine))?;
            // Render line if it exists
            let idx = y as usize + self.doc().offset.y;
            if let Some(line) = self.doc().line(idx) {
                let tokens = self.highlighter().line(idx, &line);
                let tokens = trim(&tokens, self.doc().offset.x);
                let mut x_pos = self.doc().offset.x;
                for token in tokens {
                    let text = match token {
                        TokOpt::Some(text, kind) => {
                            // Try to get the corresponding colour for this token
                            let colour = self.config.syntax_highlighting.borrow().get_theme(&kind);
                            match colour {
                                // Success, write token
                                Ok(col) => {
                                    write!(self.terminal.stdout, "{}", Fg(col),)?;
                                }
                                // Failure, show error message and don't highlight this token
                                Err(err) => {
                                    self.feedback = Feedback::Error(err.to_string());
                                }
                            }
                            text
                        }
                        TokOpt::None(text) => text,
                    };
                    for c in text.chars() {
                        let is_selected = self.doc().is_loc_selected(Loc { y: idx, x: x_pos });
                        if is_selected {
                            write!(
                                self.terminal.stdout,
                                "{}",
                                Bg(self.config.colors.borrow().selection_bg.to_color()?)
                            )?;
                            write!(
                                self.terminal.stdout,
                                "{}",
                                Fg(self.config.colors.borrow().selection_fg.to_color()?)
                            )?;
                        } else {
                            write!(
                                self.terminal.stdout,
                                "{}",
                                Bg(self.config.colors.borrow().editor_bg.to_color()?)
                            )?;
                        }
                        write!(self.terminal.stdout, "{c}")?;
                        x_pos += 1;
                    }
                    write!(
                        self.terminal.stdout,
                        "{}",
                        Fg(self.config.colors.borrow().editor_fg.to_color()?)
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Render the tab line at the top of the document
    fn render_tab_line(&mut self, w: usize) -> Result<()> {
        self.terminal.goto(0 as usize, 0 as usize)?;
        write!(
            self.terminal.stdout,
            "{}{}",
            Fg(self.config.colors.borrow().tab_inactive_fg.to_color()?),
            Bg(self.config.colors.borrow().tab_inactive_bg.to_color()?)
        )?;
        for (c, document) in self.doc.iter().enumerate() {
            let document_header = self.config.tab_line.borrow().render(document);
            if c == self.ptr {
                // Representing the document we're currently looking at
                write!(
                    self.terminal.stdout,
                    "{}{}{}{document_header}{}{}{}│",
                    Bg(self.config.colors.borrow().tab_active_bg.to_color()?),
                    Fg(self.config.colors.borrow().tab_active_fg.to_color()?),
                    SetAttribute(Attribute::Bold),
                    SetAttribute(Attribute::Reset),
                    Fg(self.config.colors.borrow().tab_inactive_fg.to_color()?),
                    Bg(self.config.colors.borrow().tab_inactive_bg.to_color()?),
                )?;
            } else {
                // Other document that is currently open
                write!(self.terminal.stdout, "{document_header}│")?;
            }
        }
        write!(self.terminal.stdout, "{}", " ".to_string().repeat(w))?;
        Ok(())
    }

    /// Render the status line at the bottom of the document
    fn render_status_line(&mut self, lua: &Lua, w: usize, h: usize) -> Result<()> {
        self.terminal.goto(0, h + self.push_down)?;
        write!(
            self.terminal.stdout,
            "{}{}{}{}{}{}{}",
            Bg(self.config.colors.borrow().status_bg.to_color()?),
            Fg(self.config.colors.borrow().status_fg.to_color()?),
            SetAttribute(Attribute::Bold),
            self.config.status_line.borrow().render(&self, &lua, w),
            SetAttribute(Attribute::Reset),
            Fg(self.config.colors.borrow().editor_fg.to_color()?),
            Bg(self.config.colors.borrow().editor_bg.to_color()?),
        )?;
        Ok(())
    }

    /// Render the feedback line
    fn render_feedback_line(&mut self, w: usize, h: usize) -> Result<()> {
        self.terminal.goto(0, h + 2)?;
        write!(
            self.terminal.stdout,
            "{}",
            self.feedback.render(&self.config.colors.borrow(), w)?,
        )?;
        Ok(())
    }

    /// Render the greeting message
    fn render_help_message(&mut self, w: usize, h: usize) -> Result<()> {
        let color = self.config.colors.borrow().highlight.to_color()?;
        let editor_fg = self.config.colors.borrow().editor_fg.to_color()?;
        let message: Vec<&str> = HELP_TEXT.split('\n').collect();
        for (c, line) in message.iter().enumerate().take(h.saturating_sub(h / 4)) {
            self.terminal.goto(w.saturating_sub(30), h / 4 + c + 1)?;
            write!(self.terminal.stdout, "{}{line}{}", Fg(color), Fg(editor_fg))?;
        }
        Ok(())
    }

    /// Render the help message
    fn render_greeting(&mut self, w: usize, h: usize) -> Result<()> {
        let colors = self.config.colors.borrow();
        let greeting = self.config.greeting_message.borrow().render(&colors)?;
        let message: Vec<&str> = greeting.split('\n').collect();
        for (c, line) in message.iter().enumerate().take(h.saturating_sub(h / 4)) {
            self.terminal.goto(4, h / 4 + c + 1)?;
            write!(
                self.terminal.stdout,
                "{}",
                alinio::align::center(&line, w.saturating_sub(4)).unwrap_or_else(|| "".to_string()),
            )?;
        }
        Ok(())
    }

    /// Display a prompt in the document
    pub fn prompt<S: Into<String>>(&mut self, prompt: S) -> Result<String> {
        let prompt = prompt.into();
        let mut input = String::new();
        let mut done = false;
        // Enter into a menu that asks for a prompt
        while !done {
            let h = size()?.h;
            let w = size()?.w;
            // Render prompt message
            self.terminal.prepare_line(h)?;
            write!(
                self.terminal.stdout,
                "{}",
                Bg(self.config.colors.borrow().editor_bg.to_color()?)
            )?;
            write!(
                self.terminal.stdout,
                "{}: {}{}",
                prompt,
                input,
                " ".to_string().repeat(w)
            )?;
            self.terminal.goto(prompt.len() + input.len() + 2, h)?;
            self.terminal.flush()?;
            // Handle events
            if let CEvent::Key(key) = read()? {
                match (key.modifiers, key.code) {
                    // Exit the menu when the enter key is pressed
                    (KMod::NONE, KCode::Enter) => done = true,
                    // Remove from the input string if the user presses backspace
                    (KMod::NONE, KCode::Backspace) => {
                        input.pop();
                    }
                    // Add to the input string if the user presses a character
                    (KMod::NONE | KMod::SHIFT, KCode::Char(c)) => input.push(c),
                    _ => (),
                }
            }
        }
        // Return input string result
        Ok(input)
    }

    /// Work out how much to push the document to the right (to make way for line numbers)
    fn dent(&self) -> usize {
        if self.config.line_numbers.borrow().enabled {
            let padding_left = self.config.line_numbers.borrow().padding_left;
            let padding_right = self.config.line_numbers.borrow().padding_right;
            self.doc().len_lines().to_string().len() + 1 + padding_left + padding_right
        } else {
            0
        }
    }

    /// Move to the next document opened in the editor
    pub fn next(&mut self) {
        if self.ptr + 1 < self.doc.len() {
            self.ptr += 1;
        }
    }

    /// Move to the previous document opened in the editor
    pub fn prev(&mut self) {
        if self.ptr != 0 {
            self.ptr = self.ptr.saturating_sub(1);
        }
    }

    /// Copy the selected text
    pub fn copy(&mut self) -> Result<()> {
        let selected_text = self.doc().selection_text();
        self.terminal.copy(&selected_text)
    }

    /// Cut the selected text
    pub fn cut(&mut self) -> Result<()> {
        self.copy()?;
        self.doc_mut().remove_selection();
        self.reload_highlight();
        Ok(())
    }

    /// Move the cursor up
    pub fn select_up(&mut self) {
        self.doc_mut().select_up();
    }

    /// Move the cursor down
    pub fn select_down(&mut self) {
        self.doc_mut().select_down();
    }

    /// Move the cursor left
    pub fn select_left(&mut self) {
        let status = self.doc_mut().select_left();
        // Cursor wrapping if cursor hits the start of the line
        let wrapping = self.config.document.borrow().wrap_cursor;
        if status == Status::StartOfLine && self.doc().loc().y != 0 && wrapping {
            self.doc_mut().select_up();
            self.doc_mut().select_end();
        }
    }

    /// Move the cursor right
    pub fn select_right(&mut self) {
        let status = self.doc_mut().select_right();
        // Cursor wrapping if cursor hits the end of a line
        let wrapping = self.config.document.borrow().wrap_cursor;
        if status == Status::EndOfLine && wrapping {
            self.doc_mut().select_down();
            self.doc_mut().select_home();
        }
    }

    /// Select the whole document
    pub fn select_all(&mut self) {
        self.doc_mut().move_top();
        self.doc_mut().select_bottom();
    }

    /// Move the cursor up
    pub fn up(&mut self) {
        self.doc_mut().move_up();
    }

    /// Move the cursor down
    pub fn down(&mut self) {
        self.doc_mut().move_down();
    }

    /// Move the cursor left
    pub fn left(&mut self) {
        let status = self.doc_mut().move_left();
        // Cursor wrapping if cursor hits the start of the line
        let wrapping = self.config.document.borrow().wrap_cursor;
        if status == Status::StartOfLine && self.doc().loc().y != 0 && wrapping {
            self.doc_mut().move_up();
            self.doc_mut().move_end();
        }
    }

    /// Move the cursor right
    pub fn right(&mut self) {
        let status = self.doc_mut().move_right();
        // Cursor wrapping if cursor hits the end of a line
        let wrapping = self.config.document.borrow().wrap_cursor;
        if status == Status::EndOfLine && wrapping {
            self.doc_mut().move_down();
            self.doc_mut().move_home();
        }
    }

    /// Move the cursor to the previous word in the line
    pub fn prev_word(&mut self) {
        let status = self.doc_mut().move_prev_word();
        let wrapping = self.config.document.borrow().wrap_cursor;
        if status == Status::StartOfLine && wrapping {
            self.doc_mut().move_up();
            self.doc_mut().move_end();
        }
    }

    /// Move the cursor to the next word in the line
    pub fn next_word(&mut self) {
        let status = self.doc_mut().move_next_word();
        let wrapping = self.config.document.borrow().wrap_cursor;
        if status == Status::EndOfLine && wrapping {
            self.doc_mut().move_down();
            self.doc_mut().move_home();
        }
    }

    /// Insert a character into the document, creating a new row if editing
    /// on the last line of the document
    pub fn character(&mut self, ch: char) -> Result<()> {
        if !self.doc().is_selection_empty() {
            self.doc_mut().remove_selection();
            self.reload_highlight();
        }
        self.new_row()?;
        // Handle the character insertion
        if ch == '\n' {
            self.enter()?;
        } else {
            let loc = self.doc().char_loc();
            self.exe(Event::Insert(loc, ch.to_string()))?;
            self.highlighter[self.ptr].edit(loc.y, &self.doc[self.ptr].lines[loc.y]);
        }
        // Commit to event stack (for undo/redo if the character is a space)
        if ch == ' ' {
            self.doc_mut().commit();
        }
        Ok(())
    }

    /// Handle the return key
    pub fn enter(&mut self) -> Result<()> {
        // When the return key is pressed, we want to commit to the undo/redo stack
        self.doc_mut().commit();
        // Perform the changes
        if self.doc().loc().y != self.doc().len_lines() {
            // Enter pressed in the start, middle or end of the line
            let loc = self.doc().char_loc();
            self.exe(Event::SplitDown(loc))?;
            let line = &self.doc[self.ptr].lines[loc.y + 1];
            self.highlighter[self.ptr].insert_line(loc.y + 1, line);
            let line = &self.doc[self.ptr].lines[loc.y];
            self.highlighter[self.ptr].edit(loc.y, line);
        } else {
            // Enter pressed on the empty line at the bottom of the document
            self.new_row()?;
        }
        Ok(())
    }

    /// Handle the backspace key
    pub fn backspace(&mut self) -> Result<()> {
        if !self.doc().is_selection_empty() {
            self.doc_mut().commit();
            self.doc_mut().undo_mgmt.set_dirty();
            self.doc_mut().remove_selection();
            // Removing a selection is significant and worth an undo commit
            self.reload_highlight();
            return Ok(());
        }
        let mut c = self.doc().char_ptr;
        let on_first_line = self.doc().loc().y == 0;
        let out_of_range = self.doc().out_of_range(0, self.doc().loc().y).is_err();
        if c == 0 && !on_first_line && !out_of_range {
            // Backspace was pressed on the start of the line, move line to the top
            self.new_row()?;
            let mut loc = self.doc().char_loc();
            self.highlighter().remove_line(loc.y);
            loc.y = loc.y.saturating_sub(1);
            loc.x = self.doc().line(loc.y).unwrap().chars().count();
            self.exe(Event::SpliceUp(loc))?;
            let line = &self.doc[self.ptr].lines[loc.y];
            self.highlighter[self.ptr].edit(loc.y, line);
        } else {
            // Backspace was pressed in the middle of the line, delete the character
            c = c.saturating_sub(1);
            if let Some(line) = self.doc().line(self.doc().loc().y) {
                if let Some(ch) = line.chars().nth(c) {
                    let loc = Loc {
                        x: c,
                        y: self.doc().loc().y,
                    };
                    self.exe(Event::Delete(loc, ch.to_string()))?;
                    self.highlighter[self.ptr].edit(loc.y, &self.doc[self.ptr].lines[loc.y]);
                }
            }
        }
        Ok(())
    }

    /// Delete the character in place
    pub fn delete(&mut self) -> Result<()> {
        let c = self.doc().char_ptr;
        if let Some(line) = self.doc().line(self.doc().loc().y) {
            if let Some(ch) = line.chars().nth(c) {
                let loc = Loc {
                    x: c,
                    y: self.doc().loc().y,
                };
                self.exe(Event::Delete(loc, ch.to_string()))?;
                self.highlighter[self.ptr].edit(loc.y, &self.doc[self.ptr].lines[loc.y]);
            }
        }
        Ok(())
    }

    /// Insert a new row at the end of the document if the cursor is on it
    fn new_row(&mut self) -> Result<()> {
        if self.doc().loc().y == self.doc().len_lines() {
            self.exe(Event::InsertLine(self.doc().loc().y, "".to_string()))?;
            self.highlighter().append(&"".to_string());
        }
        Ok(())
    }

    /// Delete the current line
    pub fn delete_line(&mut self) -> Result<()> {
        // Commit events to event manager (for undo / redo)
        self.doc_mut().commit();
        // Delete the line
        if self.doc().loc().y < self.doc().len_lines() {
            let y = self.doc().loc().y;
            let line = self.doc().line(y).unwrap();
            self.exe(Event::DeleteLine(y, line))?;
            self.highlighter().remove_line(y);
        }
        Ok(())
    }

    /// Use search feature
    pub fn search(&mut self) -> Result<()> {
        // Prompt for a search term
        let target = self.prompt("Search")?;
        let mut done = false;
        let Size { w, h } = size()?;
        // Jump to the next match after search term is provided
        self.next_match(&target);
        // Enter into search menu
        while !done {
            // Render just the document part
            self.terminal.hide_cursor()?;
            self.render_document(w, h.saturating_sub(2))?;
            // Render custom status line with mode information
            self.terminal.goto(0, h)?;
            write!(
                self.terminal.stdout,
                "[<-]: Search previous | [->]: Search next"
            )?;
            self.terminal.flush()?;
            // Move back to correct cursor position
            if let Some(Loc { x, y }) = self.doc().cursor_loc_in_screen() {
                let max = self.dent();
                self.terminal.goto(x + max, y + 1)?;
                self.terminal.show_cursor()?;
            } else {
                self.terminal.hide_cursor()?;
            }
            // Handle events
            if let CEvent::Key(key) = read()? {
                match (key.modifiers, key.code) {
                    // On return or escape key, exit menu
                    (KMod::NONE, KCode::Enter | KCode::Esc) => done = true,
                    // On left key, move to the previous match in the document
                    (KMod::NONE, KCode::Left) => std::mem::drop(self.prev_match(&target)),
                    // On right key, move to the next match in the document
                    (KMod::NONE, KCode::Right) => std::mem::drop(self.next_match(&target)),
                    _ => (),
                }
            }
            self.update_highlighter()?;
        }
        Ok(())
    }

    /// Move to the next match
    pub fn next_match(&mut self, target: &str) -> Option<String> {
        let mtch = self.doc_mut().next_match(target, 1)?;
        self.doc_mut().move_to(&mtch.loc);
        // Update highlighting
        self.update_highlighter().ok()?;
        Some(mtch.text)
    }

    /// Move to the previous match
    pub fn prev_match(&mut self, target: &str) -> Option<String> {
        let mtch = self.doc_mut().prev_match(target)?;
        self.doc_mut().move_to(&mtch.loc);
        // Update highlighting
        self.update_highlighter().ok()?;
        Some(mtch.text)
    }

    /// Use replace feature
    pub fn replace(&mut self) -> Result<()> {
        // Request replace information
        let target = self.prompt("Replace")?;
        let into = self.prompt("With")?;
        let mut done = false;
        let Size { w, h } = size()?;
        // Jump to match
        let mut mtch;
        if let Some(m) = self.next_match(&target) {
            // Automatically move to next match, keeping note of what that match is
            mtch = m;
        } else if let Some(m) = self.prev_match(&target) {
            // Automatically move to previous match, keeping not of what that match is
            // This happens if there are no matches further down the document, only above
            mtch = m;
        } else {
            // Exit if there are no matches in the document
            return Ok(());
        }
        self.update_highlighter()?;
        // Enter into the replace menu
        while !done {
            // Render just the document part
            self.terminal.hide_cursor()?;
            self.render_document(w, h.saturating_sub(2))?;
            // Write custom status line for the replace mode
            self.terminal.goto(0, h)?;
            write!(
                self.terminal.stdout,
                "[<-] Previous | [->] Next | [Enter] Replace | [Tab] Replace All"
            )?;
            self.terminal.flush()?;
            // Move back to correct cursor location
            if let Some(Loc { x, y }) = self.doc().cursor_loc_in_screen() {
                let max = self.dent();
                self.terminal.goto(x + max, y + 1)?;
                self.terminal.show_cursor()?;
            } else {
                self.terminal.hide_cursor()?;
            }
            // Handle events
            if let CEvent::Key(key) = read()? {
                match (key.modifiers, key.code) {
                    // On escape key, exit
                    (KMod::NONE, KCode::Esc) => done = true,
                    // On right key, move to the previous match, keeping note of what that match is
                    (KMod::NONE, KCode::Left) => mtch = self.prev_match(&target).unwrap_or(mtch),
                    // On left key, move to the next match, keeping note of what that match is
                    (KMod::NONE, KCode::Right) => mtch = self.next_match(&target).unwrap_or(mtch),
                    // On return key, perform replacement
                    (KMod::NONE, KCode::Enter) => self.do_replace(&into, &mtch)?,
                    // On tab key, replace all instances within the document
                    (KMod::NONE, KCode::Tab) => self.do_replace_all(&target, &into),
                    _ => (),
                }
            }
            // Update syntax highlighter if necessary
            self.update_highlighter()?;
        }
        Ok(())
    }

    /// Replace an instance in a document
    fn do_replace(&mut self, into: &str, text: &str) -> Result<()> {
        // Commit events to event manager (for undo / redo)
        self.doc_mut().commit();
        // Do the replacement
        let loc = self.doc().char_loc();
        self.doc_mut().replace(loc, text, into)?;
        self.doc_mut().move_to(&loc);
        // Update syntax highlighter
        self.update_highlighter()?;
        self.highlighter[self.ptr].edit(loc.y, &self.doc[self.ptr].lines[loc.y]);
        Ok(())
    }

    /// Replace all instances in a document
    fn do_replace_all(&mut self, target: &str, into: &str) {
        // Commit events to event manager (for undo / redo)
        self.doc_mut().commit();
        // Replace everything top to bottom
        self.doc_mut().move_to(&Loc::at(0, 0));
        while let Some(mtch) = self.doc_mut().next_match(target, 1) {
            drop(self.doc_mut().replace(mtch.loc, &mtch.text, into));
            drop(self.update_highlighter());
            self.highlighter[self.ptr].edit(mtch.loc.y, &self.doc[self.ptr].lines[mtch.loc.y]);
        }
    }

    fn reload_highlight(&mut self) {
        for (line, text) in self.doc[self.ptr].lines.iter().enumerate() {
            self.highlighter[self.ptr].edit(line, text);
        }
    }

    /// Perform redo action
    pub fn redo(&mut self) -> Result<()> {
        let result = Ok(self.doc_mut().redo()?);
        self.reload_highlight();
        result
    }

    /// Perform undo action
    pub fn undo(&mut self) -> Result<()> {
        let result = Ok(self.doc_mut().undo()?);
        self.reload_highlight();
        result
    }

    /// save the document to the disk
    pub fn save(&mut self) -> Result<()> {
        // Commit events to event manager (for undo / redo)
        self.doc_mut().commit();
        // Perform the save
        self.doc_mut().save()?;
        // All done
        self.feedback = Feedback::Info("Document saved successfully".to_string());
        Ok(())
    }

    /// save the document to the disk at a specified path
    pub fn save_as(&mut self) -> Result<()> {
        let file_name = self.prompt("Save as")?;
        self.doc_mut().save_as(&file_name)?;
        if self.doc().file_name.is_none() {
            let ext = file_name.split('.').last().unwrap_or("");
            self.highlighter[self.ptr] = self
                .config
                .syntax_highlighting
                .borrow()
                .get_highlighter(&ext);
            self.doc_mut().file_name = Some(file_name.clone());
            self.doc_mut().modified = false;
        }
        // Commit events to event manager (for undo / redo)
        self.doc_mut().commit();
        // All done
        self.feedback = Feedback::Info(format!("Document saved as {file_name} successfully"));
        Ok(())
    }

    /// Save all the open documents to the disk
    pub fn save_all(&mut self) -> Result<()> {
        for doc in self.doc.iter_mut() {
            doc.save()?;
            // Commit events to event manager (for undo / redo)
            doc.commit();
        }
        self.feedback = Feedback::Info(format!("Saved all documents"));
        Ok(())
    }

    /// Quit the editor
    pub fn quit(&mut self) -> Result<()> {
        self.active = !self.doc.is_empty();
        // If there are still documents open, only close the requested document
        if self.active {
            let msg = "This document isn't saved, press Ctrl + Q to force quit or Esc to cancel";
            if !self.doc().modified || self.confirm(msg)? {
                self.doc.remove(self.ptr);
                self.highlighter.remove(self.ptr);
                self.prev();
            }
        }
        self.active = !self.doc.is_empty();
        Ok(())
    }

    /// Confirmation dialog
    pub fn confirm(&mut self, msg: &str) -> Result<bool> {
        let mut done = false;
        let mut result = false;
        // Enter into the confirmation menu
        self.terminal.hide_cursor()?;
        while !done {
            let h = size()?.h;
            let w = size()?.w;
            // Render message
            self.feedback = Feedback::Warning(msg.to_string());
            self.render_feedback_line(w, h)?;
            self.terminal.flush()?;
            // Handle events
            if let CEvent::Key(key) = read()? {
                match (key.modifiers, key.code) {
                    // Exit the menu when the enter key is pressed
                    (KMod::NONE, KCode::Esc) => {
                        done = true;
                        self.feedback = Feedback::None;
                    }
                    // Add to the input string if the user presses a character
                    (KMod::CONTROL, KCode::Char('q')) => {
                        done = true;
                        result = true;
                        self.feedback = Feedback::None;
                    }
                    _ => (),
                }
            }
        }
        self.terminal.show_cursor()?;
        Ok(result)
    }
}
