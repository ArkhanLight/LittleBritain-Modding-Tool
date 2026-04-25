# Code Editor Component for LittleBritain Mod Tool
# Simple text editor with syntax highlighting for Lua/C++ scripts

use eframe::egui;
use std::fs;
use std::path::PathBuf;

#[derive(Default)]
pub struct CodeEditor {
    pub content: String,
    pub path: Option<PathBuf>,
    pub modified: bool,
    pub line_numbers: Vec<String>,
    pub scroll_pos: f32,
}

impl CodeEditor {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            path: None,
            modified: false,
            line_numbers: Vec::new(),
            scroll_pos: 0.0,
        }
    }
    
    pub fn load(&mut self, path: &PathBuf) -> Result<(), String> {
        match fs::read_to_string(path) {
            Ok(content) => {
                self.content = content;
                self.path = Some(path.clone());
                self.modified = false;
                self.update_line_numbers();
                Ok(())
            }
            Err(e) => Err(format!("Failed to read file: {}", e)),
        }
    }
    
    pub fn save(&mut self) -> Result<(), String> {
        if let Some(ref path) = self.path {
            fs::write(path, &self.content)
                .map_err(|e| format!("Failed to write file: {}", e))?;
            self.modified = false;
            Ok(())
        } else {
            Err("No file path set".to_string())
        }
    }
    
    pub fn save_as(&mut self, path: &PathBuf) -> Result<(), String> {
        fs::write(path, &self.content)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        self.path = Some(path.clone());
        self.modified = false;
        Ok(())
    }
    
    pub fn new_file(&mut self) {
        self.content = String::new();
        self.path = None;
        self.modified = false;
        self.update_line_numbers();
    }
    
    fn update_line_numbers(&mut self) {
        self.line_numbers.clear();
        for (i, _) in self.content.lines().enumerate() {
            self.line_numbers.push(format!("{}", i + 1));
        }
        if self.content.is_empty() || self.content.ends_with('\n') {
            self.line_numbers.push(format!("{}", self.line_numbers.len()));
        }
    }
    
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let font_size = 14.0_f32;
        let line_height = font_size * 1.4;
        
        // Toolbar
        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                self.new_file();
            }
            if ui.button("Open").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Scripts", &["lua", "cpp", "h", "txt"])
                    .pick_file() 
                {
                    let _ = self.load(&path);
                }
            }
            if ui.button("Save").clicked() {
                if self.path.is_some() {
                    let _ = self.save();
                } else if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Scripts", &["lua", "cpp", "h"])
                    .save_file() 
                {
                    let _ = self.save_as(&path);
                }
            }
            if ui.button("Save As").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Scripts", &["lua", "cpp", "h"])
                    .save_file() 
                {
                    let _ = self.save_as(&path);
                }
            }
            
            ui.separator();
            
            let title = match &self.path {
                Some(p) => {
                    let name = p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if self.modified {
                        format!("{} *", name)
                    } else {
                        name
                    }
                }
                None => "Untitled".to_string(),
            };
            ui.label(title);
        });
        
        ui.separator();
        
        // Editor area
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_size(egui::vec2(ui.available_width(), 400.0));
                
                let galley = ui.fonts().layout_single_line(
                    egui::TextStyle::Body.resolve(ui.style()),
                    "0",
                );
                let char_width = galley.size.x;
                
                // Line numbers + code
                ui.horizontal(|ui| {
                    // Line numbers
                    egui::Frame::group(ui)
                        .fill(egui::Color32::from_rgb(30, 30, 30))
                        .show(ui, |ui| {
                            ui.set_width(50.0);
                            let num_style = egui::TextStyle::Body.with_size(font_size);
                            for line_num in &self.line_numbers {
                                ui.label(
                                    egui::RichText::new(line_num)
                                        .size(font_size)
                                        .color(egui::Color32::GRAY)
                                );
                            }
                        });
                    
                    // Code text area
                    let text_edit = egui::widgets::TextEdit::multiline(&mut self.content)
                        .font(egui::TextStyle::Body.resolve(ui.style()))
                        .desired_width(ui.available_width() - 60.0)
                        .lock_focus(true);
                    
                    ui.put(
                        egui::Rect::from_min_size(
                            ui.cursor().min + egui::vec2(10.0, 0.0),
                            egui::vec2(ui.available_width() - 60.0, 600.0)
                        ),
                        text_edit,
                    );
                });
            });
        
        // Update line numbers when content changes
        self.update_line_numbers();
        
        // Status bar
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("Lines: {}", self.line_numbers.len()));
            ui.separator();
            if self.modified {
                ui.label(egui::RichText::new("Modified").color(egui::Color32::YELLOW));
            } else {
                ui.label("Saved");
            }
        });
    }
}

// Syntax highlighting (simple)
pub struct SyntaxHighlighter {
    pub keywords_lua: Vec<&'static str>,
    pub keywords_cpp: Vec<&'static str>,
    pub builtins: Vec<&'static str>,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self {
            keywords_lua: vec![
                "function", "end", "if", "then", "else", "elseif", "for", "while", "do",
                "local", "return", "nil", "true", "false", "and", "or", "not", "in",
            ],
            keywords_cpp: vec![
                "void", "int", "float", "char", "bool", "class", "struct", "public", "private",
                "virtual", "if", "else", "for", "while", "return", "namespace", "using",
                "const", "static", "inline", "extern", "sizeof", "typedef", "enum",
            ],
            builtins: vec![
                "print", "log", "require", "import", "export", "define", "include",
            ],
        }
    }
}