//! Embedded repository editor and bounded rich-file preview.

use std::path::PathBuf;

use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, markdown, row, scrollable, text_editor, text_input, Space,
};
use iced::{Alignment, Color, Element, Length, Task};
use suaegi_core::domain::WorktreeId;
use suaegi_git::fs::{
    read_editable_file, write_file, EditableFileRead, FileSignature, WriteOutcome,
};

use crate::background;
use crate::i18n::text;
use crate::state::{AppState, Message, OpId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorLoad {
    Ready {
        text: String,
        size: u64,
        signature: FileSignature,
    },
    Binary {
        size: u64,
    },
    Preview {
        bytes: Vec<u8>,
        size: u64,
        kind: PreviewKind,
    },
    TooLarge {
        limit: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Image,
    Pdf,
}

#[derive(Debug, Clone)]
pub struct EditorSave {
    pub worktree: WorktreeId,
    pub path: String,
    pub text: String,
    pub expected: FileSignature,
    pub op: OpId,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum Document {
    Closed,
    Loading {
        worktree: WorktreeId,
        path: String,
        op: OpId,
    },
    Ready {
        worktree: WorktreeId,
        path: String,
        content: text_editor::Content,
        markdown: markdown::Content,
        misspellings: Vec<String>,
        signature: FileSignature,
        edit_generation: u64,
        dirty: bool,
        saving: Option<(OpId, u64)>,
        close_confirmation: bool,
        error: Option<String>,
    },
    Unavailable {
        worktree: WorktreeId,
        path: String,
        message: String,
    },
    Preview {
        worktree: WorktreeId,
        path: String,
        bytes: Vec<u8>,
        size: u64,
        kind: PreviewKind,
    },
}

#[derive(Debug)]
pub struct EditorState {
    document: Document,
    inactive_documents: Vec<Document>,
    find_visible: bool,
    replace_visible: bool,
    find_query: String,
    replacement: String,
    find_match: Option<(usize, usize)>,
    find_status: Option<String>,
    markdown_preview: bool,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            document: Document::Closed,
            inactive_documents: Vec::new(),
            find_visible: false,
            replace_visible: false,
            find_query: String::new(),
            replacement: String::new(),
            find_match: None,
            find_status: None,
            markdown_preview: false,
        }
    }
}

pub fn find_input_id() -> iced::widget::Id {
    iced::widget::Id::new("editor-find")
}

fn byte_position(text: &str, byte: usize) -> text_editor::Position {
    let mut line = 0;
    let mut column = 0;
    for character in text[..byte.min(text.len())].chars() {
        if character == '\n' {
            line += 1;
            column = 0;
        } else if character != '\r' {
            column += 1;
        }
    }
    text_editor::Position { line, column }
}

impl EditorState {
    pub fn is_open(&self) -> bool {
        !matches!(self.document, Document::Closed)
    }

    pub fn location(&self) -> Option<(&WorktreeId, &str)> {
        match &self.document {
            Document::Loading { worktree, path, .. }
            | Document::Ready { worktree, path, .. }
            | Document::Unavailable { worktree, path, .. }
            | Document::Preview { worktree, path, .. } => Some((worktree, path)),
            Document::Closed => None,
        }
    }

    pub fn close(&mut self) {
        self.document = Document::Closed;
        self.inactive_documents.clear();
        self.close_find();
        self.markdown_preview = false;
    }

    pub fn begin_load(&mut self, worktree: WorktreeId, path: String, op: OpId) {
        self.inactive_documents
            .retain(|document| !document_is(document, &worktree, &path));
        let next = Document::Loading { worktree, path, op };
        let previous = std::mem::replace(&mut self.document, next);
        if !matches!(previous, Document::Closed) {
            self.inactive_documents.push(previous);
        }
        self.close_find();
        self.markdown_preview = false;
    }

    pub fn accept_load(
        &mut self,
        worktree: &WorktreeId,
        path: &str,
        op: OpId,
        result: Result<EditorLoad, String>,
    ) -> bool {
        let Some(document) = self.document_mut(worktree, path) else {
            return false;
        };
        if !matches!(
            document,
            Document::Loading {
                op: expected_op,
                ..
            } if *expected_op == op
        ) {
            return false;
        }
        *document = match result {
            Ok(EditorLoad::Ready {
                text, signature, ..
            }) => {
                let markdown = markdown::Content::parse(&text);
                Document::Ready {
                    worktree: worktree.clone(),
                    path: path.to_string(),
                    content: text_editor::Content::with_text(&text),
                    markdown,
                    misspellings: Vec::new(),
                    signature,
                    edit_generation: 0,
                    dirty: false,
                    saving: None,
                    close_confirmation: false,
                    error: None,
                }
            }
            Ok(EditorLoad::Binary { size }) => Document::Unavailable {
                worktree: worktree.clone(),
                path: path.to_string(),
                message: format!("Binary file ({size} bytes) cannot be edited as text."),
            },
            Ok(EditorLoad::TooLarge { limit }) => Document::Unavailable {
                worktree: worktree.clone(),
                path: path.to_string(),
                message: format!(
                    "File is larger than the {} MB editor limit.",
                    limit / (1024 * 1024)
                ),
            },
            Ok(EditorLoad::Preview { bytes, size, kind }) => Document::Preview {
                worktree: worktree.clone(),
                path: path.to_string(),
                bytes,
                size,
                kind,
            },
            Err(error) => Document::Unavailable {
                worktree: worktree.clone(),
                path: path.to_string(),
                message: error,
            },
        };
        true
    }

    pub fn activate(&mut self, worktree: &WorktreeId, path: &str) -> bool {
        if document_is(&self.document, worktree, path) {
            return true;
        }
        let Some(index) = self
            .inactive_documents
            .iter()
            .position(|document| document_is(document, worktree, path))
        else {
            return false;
        };
        std::mem::swap(&mut self.document, &mut self.inactive_documents[index]);
        self.close_find();
        self.markdown_preview = false;
        true
    }

    pub fn tabs(&self) -> Vec<EditorTab> {
        self.inactive_documents
            .iter()
            .map(|document| EditorTab::from_document(document, false))
            .chain(
                (!matches!(self.document, Document::Closed))
                    .then(|| EditorTab::from_document(&self.document, true)),
            )
            .collect()
    }

    pub fn perform(&mut self, action: text_editor::Action) {
        let Document::Ready {
            content,
            markdown,
            edit_generation,
            dirty,
            close_confirmation,
            error,
            ..
        } = &mut self.document
        else {
            return;
        };
        let is_edit = action.is_edit();
        content.perform(action);
        if is_edit {
            *markdown = markdown::Content::parse(&content.text());
            *edit_generation = edit_generation.saturating_add(1);
            *dirty = true;
            *close_confirmation = false;
            *error = None;
            self.find_match = None;
        }
    }

    pub fn refresh_spellcheck(&mut self, enabled: bool) {
        let is_markdown = self.is_markdown();
        let Document::Ready {
            content,
            misspellings,
            ..
        } = &mut self.document
        else {
            return;
        };
        *misspellings = if enabled && is_markdown {
            crate::spellcheck::misspellings(&content.text())
        } else {
            Vec::new()
        };
    }

    pub fn misspellings(&self) -> &[String] {
        match &self.document {
            Document::Ready { misspellings, .. } => misspellings,
            _ => &[],
        }
    }

    pub fn open_find(&mut self, replace: bool) {
        if !matches!(self.document, Document::Ready { .. }) {
            return;
        }
        self.find_visible = true;
        self.replace_visible = replace;
        self.find_status = None;
    }

    pub fn close_find(&mut self) {
        self.find_visible = false;
        self.replace_visible = false;
        self.find_match = None;
        self.find_status = None;
    }

    pub fn set_find_query(&mut self, query: String) {
        self.find_query = query;
        self.find_match = None;
        self.find_status = None;
    }

    pub fn set_replacement(&mut self, replacement: String) {
        self.replacement = replacement;
    }

    pub fn find_next(&mut self, backwards: bool) -> bool {
        let Document::Ready { content, .. } = &mut self.document else {
            return false;
        };
        if self.find_query.is_empty() {
            self.find_status = Some("Enter text to find.".into());
            return false;
        }
        let source = content.text();
        let found = if backwards {
            let before = self.find_match.map_or(source.len(), |(start, _)| start);
            source[..before]
                .rfind(&self.find_query)
                .or_else(|| source.rfind(&self.find_query))
        } else {
            let after = self.find_match.map_or(0, |(_, end)| end);
            source[after..]
                .find(&self.find_query)
                .map(|offset| after + offset)
                .or_else(|| source.find(&self.find_query))
        };
        let Some(start) = found else {
            self.find_match = None;
            self.find_status = Some("No matches.".into());
            return false;
        };
        let end = start + self.find_query.len();
        content.move_to(text_editor::Cursor {
            position: byte_position(&source, end),
            selection: Some(byte_position(&source, start)),
        });
        self.find_match = Some((start, end));
        let total = source.matches(&self.find_query).count();
        self.find_status = Some(format!(
            "{total} match{}",
            if total == 1 { "" } else { "es" }
        ));
        true
    }

    pub fn replace_current(&mut self) -> bool {
        let Some((start, end)) = self.find_match else {
            return self.find_next(false);
        };
        let Document::Ready { content, .. } = &self.document else {
            return false;
        };
        let source = content.text();
        if source.get(start..end) != Some(self.find_query.as_str()) {
            self.find_match = None;
            return self.find_next(false);
        }
        if let Document::Ready { content, .. } = &mut self.document {
            content.move_to(text_editor::Cursor {
                position: byte_position(&source, end),
                selection: Some(byte_position(&source, start)),
            });
        }
        let replacement = self.replacement.clone();
        self.perform(text_editor::Action::Edit(text_editor::Edit::Paste(
            std::sync::Arc::new(replacement),
        )));
        self.find_match = Some((start, start + self.replacement.len()));
        self.find_next(false);
        true
    }

    pub fn replace_all(&mut self) -> usize {
        if self.find_query.is_empty() {
            self.find_status = Some("Enter text to find.".into());
            return 0;
        }
        let Document::Ready { content, .. } = &self.document else {
            return 0;
        };
        let source = content.text();
        let count = source.matches(&self.find_query).count();
        if count == 0 {
            self.find_status = Some("No matches.".into());
            return 0;
        }
        let replaced = source.replace(&self.find_query, &self.replacement);
        let Document::Ready {
            content,
            markdown,
            edit_generation,
            dirty,
            close_confirmation,
            error,
            ..
        } = &mut self.document
        else {
            return 0;
        };
        *content = text_editor::Content::with_text(&replaced);
        *markdown = markdown::Content::parse(&replaced);
        *edit_generation = edit_generation.saturating_add(1);
        *dirty = true;
        *close_confirmation = false;
        *error = None;
        self.find_match = None;
        self.find_status = Some(format!("Replaced {count}."));
        count
    }

    pub fn has_unsaved_changes(&self) -> bool {
        matches!(&self.document, Document::Ready { dirty: true, .. })
    }

    pub fn is_markdown(&self) -> bool {
        let Some((_, path)) = self.location() else {
            return false;
        };
        matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("md" | "markdown" | "mdown" | "mkd")
        )
    }

    pub fn markdown_preview(&self) -> bool {
        self.markdown_preview && self.is_markdown()
    }

    pub fn toggle_markdown_preview(&mut self) {
        if self.is_markdown() {
            self.markdown_preview = !self.markdown_preview;
            self.close_find();
        }
    }

    pub fn is_document(&self, worktree: &WorktreeId, path: &str) -> bool {
        document_is(&self.document, worktree, path)
            || self
                .inactive_documents
                .iter()
                .any(|document| document_is(document, worktree, path))
    }

    /// Returns `true` when the document was closed. Dirty buffers require an
    /// explicit second action so a panel toggle or accidental click cannot
    /// discard edits.
    pub fn request_close(&mut self) -> bool {
        if let Document::Ready {
            dirty: true,
            close_confirmation,
            ..
        } = &mut self.document
        {
            *close_confirmation = true;
            return false;
        }
        self.close_active();
        true
    }

    pub fn cancel_close(&mut self) {
        if let Document::Ready {
            close_confirmation, ..
        } = &mut self.document
        {
            *close_confirmation = false;
        }
    }

    pub fn discard_and_close(&mut self) {
        self.close_active();
    }

    pub fn begin_save(&mut self, op: OpId) -> Option<EditorSave> {
        let Document::Ready {
            worktree,
            path,
            content,
            signature,
            edit_generation,
            dirty,
            saving,
            close_confirmation,
            error,
            ..
        } = &mut self.document
        else {
            return None;
        };
        if !*dirty || saving.is_some() {
            return None;
        }
        *saving = Some((op, *edit_generation));
        *close_confirmation = false;
        *error = None;
        Some(EditorSave {
            worktree: worktree.clone(),
            path: path.clone(),
            text: content.text(),
            expected: signature.clone(),
            op,
        })
    }

    pub fn accept_save(
        &mut self,
        worktree: &WorktreeId,
        path: &str,
        op: OpId,
        result: Result<WriteOutcome, String>,
    ) -> bool {
        let Some(document) = self.document_mut(worktree, path) else {
            return false;
        };
        let Document::Ready {
            worktree: expected_worktree,
            path: expected_path,
            signature,
            edit_generation,
            dirty,
            saving,
            close_confirmation,
            error,
            ..
        } = document
        else {
            return false;
        };
        if expected_worktree != worktree || expected_path != path {
            return false;
        }
        let Some((expected_op, saved_generation)) = *saving else {
            return false;
        };
        if expected_op != op {
            return false;
        }
        *saving = None;
        match result {
            Ok(WriteOutcome::Written {
                signature: new_signature,
            }) => {
                *signature = new_signature;
                // The user may have kept typing while the save was in flight.
                // Only the exact snapshot that landed is clean.
                *dirty = *edit_generation != saved_generation;
                *close_confirmation = false;
                *error = None;
            }
            Ok(WriteOutcome::StaleConflict { .. }) => {
                *dirty = true;
                *error =
                    Some("The file changed on disk. Reload it before saving again.".to_string());
            }
            Err(message) => {
                *dirty = true;
                *error = Some(message);
            }
        }
        true
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        match &self.document {
            Document::Closed => None,
            Document::Loading { worktree, .. }
            | Document::Ready { worktree, .. }
            | Document::Unavailable { worktree, .. }
            | Document::Preview { worktree, .. } => Some(worktree),
        }
    }

    pub fn contains_worktree(&self, worktree: &WorktreeId) -> bool {
        self.worktree() == Some(worktree)
            || self
                .inactive_documents
                .iter()
                .any(|document| document_location(document).is_some_and(|(id, _)| id == worktree))
    }

    pub fn close_worktree(&mut self, worktree: &WorktreeId) {
        self.inactive_documents.retain(|document| {
            document_location(document).is_none_or(|(candidate, _)| candidate != worktree)
        });
        if self.worktree() == Some(worktree) {
            self.close_active();
        }
    }

    pub fn external_watch_target(&self) -> Option<(WorktreeId, String, FileSignature)> {
        let Document::Ready {
            worktree,
            path,
            signature,
            dirty,
            saving,
            ..
        } = &self.document
        else {
            return None;
        };
        (!*dirty && saving.is_none()).then(|| (worktree.clone(), path.clone(), signature.clone()))
    }

    pub fn is_clean_version(
        &self,
        worktree: &WorktreeId,
        path: &str,
        signature: &FileSignature,
    ) -> bool {
        matches!(
            &self.document,
            Document::Ready {
                worktree: current_worktree,
                path: current_path,
                signature: current_signature,
                dirty: false,
                saving: None,
                ..
            } if current_worktree == worktree
                && current_path == path
                && current_signature == signature
        )
    }

    #[cfg(test)]
    fn ready_snapshot(&self) -> Option<(String, bool, bool, Option<&str>)> {
        let Document::Ready {
            content,
            dirty,
            saving,
            error,
            ..
        } = &self.document
        else {
            return None;
        };
        Some((content.text(), *dirty, saving.is_some(), error.as_deref()))
    }

    fn document_mut(&mut self, worktree: &WorktreeId, path: &str) -> Option<&mut Document> {
        if document_is(&self.document, worktree, path) {
            return Some(&mut self.document);
        }
        self.inactive_documents
            .iter_mut()
            .find(|document| document_is(document, worktree, path))
    }

    fn close_active(&mut self) {
        self.document = self.inactive_documents.pop().unwrap_or(Document::Closed);
        self.close_find();
        self.markdown_preview = false;
    }
}

#[derive(Debug, Clone)]
pub struct EditorTab {
    pub worktree: WorktreeId,
    pub path: String,
    pub dirty: bool,
    pub saving: bool,
    pub active: bool,
}

impl EditorTab {
    fn from_document(document: &Document, active: bool) -> Self {
        let (worktree, path) =
            document_location(document).expect("closed documents are never exposed as tabs");
        let (dirty, saving) = match document {
            Document::Ready { dirty, saving, .. } => (*dirty, saving.is_some()),
            _ => (false, false),
        };
        Self {
            worktree: worktree.clone(),
            path: path.to_string(),
            dirty,
            saving,
            active,
        }
    }
}

fn document_location(document: &Document) -> Option<(&WorktreeId, &str)> {
    match document {
        Document::Loading { worktree, path, .. }
        | Document::Ready { worktree, path, .. }
        | Document::Unavailable { worktree, path, .. }
        | Document::Preview { worktree, path, .. } => Some((worktree, path)),
        Document::Closed => None,
    }
}

fn document_is(document: &Document, worktree: &WorktreeId, path: &str) -> bool {
    document_location(document).is_some_and(|(candidate_worktree, candidate_path)| {
        candidate_worktree == worktree && candidate_path == path
    })
}

pub fn load_file_now(worktree: PathBuf, path: String) -> Result<EditorLoad, String> {
    match read_editable_file(&worktree, &path).map_err(|error| error.to_string())? {
        EditableFileRead::Ready {
            text,
            size,
            signature,
        } => Ok(EditorLoad::Ready {
            text,
            size,
            signature,
        }),
        EditableFileRead::Binary { bytes, size } => Ok(binary_preview(&path, bytes, size)),
        EditableFileRead::TooLarge { limit } => Ok(EditorLoad::TooLarge { limit }),
    }
}

pub async fn file_signature_now(
    worktree: PathBuf,
    path: String,
    expected: FileSignature,
) -> Result<FileSignature, String> {
    tokio::task::spawn_blocking(move || {
        suaegi_git::fs::file_signature_for_compare(&worktree, &path, &expected)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("File signature worker failed: {error}"))?
}

pub(crate) fn binary_preview(path: &str, bytes: Vec<u8>, size: u64) -> EditorLoad {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(
        extension.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff")
    ) {
        return EditorLoad::Preview {
            bytes,
            size,
            kind: PreviewKind::Image,
        };
    }
    if extension.as_deref() == Some("pdf") {
        if let Some(thumbnail) = pdf_thumbnail(&bytes) {
            return EditorLoad::Preview {
                bytes: thumbnail,
                size,
                kind: PreviewKind::Pdf,
            };
        }
    }
    EditorLoad::Binary { size }
}

pub(crate) async fn binary_preview_async(path: String, bytes: Vec<u8>, size: u64) -> EditorLoad {
    tokio::task::spawn_blocking(move || binary_preview(&path, bytes, size))
        .await
        .unwrap_or(EditorLoad::Binary { size })
}

fn pdf_thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use wait_timeout::ChildExt as _;

    let directory = tempfile::tempdir().ok()?;
    let input = directory.path().join("document.pdf");
    std::fs::write(&input, bytes).ok()?;
    let (mut child, output) = if cfg!(target_os = "macos") {
        (
            Command::new("/usr/bin/qlmanage")
                .args(["-t", "-s", "1400", "-o"])
                .arg(directory.path())
                .arg(&input)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()?,
            directory.path().join("document.pdf.png"),
        )
    } else {
        let output = directory.path().join("document");
        (
            Command::new("pdftoppm")
                .args(["-png", "-f", "1", "-singlefile"])
                .arg(&input)
                .arg(&output)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()?,
            output.with_extension("png"),
        )
    };
    let status = child.wait_timeout(Duration::from_secs(10)).ok()??;
    if !status.success() {
        return None;
    }
    let thumbnail = std::fs::read(output).ok()?;
    (thumbnail.len() <= 24 * 1024 * 1024 && thumbnail.starts_with(b"\x89PNG\r\n\x1a\n"))
        .then_some(thumbnail)
}

pub fn load_file(
    worktree_id: WorktreeId,
    worktree_path: PathBuf,
    path: String,
    op: OpId,
) -> Task<Message> {
    let worktree_for_message = worktree_id.clone();
    let path_for_message = path.clone();
    background::blocking(move |mut sender| {
        let result = load_file_now(worktree_path, path);
        let _ = sender.try_send(Message::EditorFileLoaded {
            worktree: worktree_for_message,
            path: path_for_message,
            op,
            result,
        });
    })
}

pub fn save_file_now(worktree: PathBuf, save: &EditorSave) -> Result<WriteOutcome, String> {
    write_file(
        &worktree,
        &save.path,
        save.text.as_bytes(),
        Some(&save.expected),
    )
    .map_err(|error| error.to_string())
}

pub fn save_file(worktree_path: PathBuf, save: EditorSave) -> Task<Message> {
    background::blocking(move |mut sender| {
        let result = save_file_now(worktree_path, &save);
        let _ = sender.try_send(Message::EditorFileSaved {
            worktree: save.worktree,
            path: save.path,
            op: save.op,
            result,
        });
    })
}

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    let editor = state.editor();
    let body: Element<'_, Message> = match &editor.document {
        Document::Closed => return None,
        Document::Loading { .. } => column![
            editor_header(state, false, false, false, false, false),
            container(text("Opening file…").size(14)).padding(10)
        ]
        .into(),
        Document::Unavailable { message, .. } => column![
            editor_header(state, false, false, false, false, false),
            container(
                text(message)
                    .size(14)
                    .color(Color::from_rgb(0.75, 0.22, 0.17))
            )
            .padding(10)
        ]
        .into(),
        Document::Preview {
            bytes, size, kind, ..
        } => {
            let label = match kind {
                PreviewKind::Image => format!("Image · {size} bytes"),
                PreviewKind::Pdf => format!("PDF first-page preview · {size} bytes"),
            };
            column![
                editor_header(state, false, false, false, false, false),
                container(
                    column![
                        iced::widget::image(iced::widget::image::Handle::from_bytes(bytes.clone()))
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced::ContentFit::Contain),
                        text(label).size(11).color(crate::theme::MUTED),
                    ]
                    .spacing(6)
                    .align_x(Alignment::Center)
                )
                .padding(12)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill),
            ]
            .into()
        }
        Document::Ready {
            content,
            markdown,
            dirty,
            saving,
            close_confirmation,
            error,
            ..
        } => {
            let markdown_tools = state.ui_settings().markdown_review_tools && editor.is_markdown();
            let markdown_preview = markdown_tools && editor.markdown_preview();
            let find_bar = editor.find_visible.then(|| {
                let find = text_input("Find", &editor.find_query)
                    .id(find_input_id())
                    .on_input(Message::EditorFindQueryChanged)
                    .on_submit(Message::EditorFindNext(false))
                    .padding([4, 7])
                    .size(12)
                    .width(Length::Fixed(220.0));
                let mut controls = row![
                    find,
                    button("↑")
                        .on_press(Message::EditorFindNext(true))
                        .padding([3, 6])
                        .style(crate::theme::ghost_button),
                    button("↓")
                        .on_press(Message::EditorFindNext(false))
                        .padding([3, 6])
                        .style(crate::theme::ghost_button),
                ]
                .spacing(4)
                .align_y(Alignment::Center);
                if editor.replace_visible {
                    controls = controls
                        .push(
                            text_input("Replace", &editor.replacement)
                                .on_input(Message::EditorReplacementChanged)
                                .padding([4, 7])
                                .size(12)
                                .width(Length::Fixed(220.0)),
                        )
                        .push(
                            button("Replace")
                                .on_press(Message::EditorReplaceCurrent)
                                .padding([3, 7])
                                .style(crate::theme::ghost_button),
                        )
                        .push(
                            button("All")
                                .on_press(Message::EditorReplaceAll)
                                .padding([3, 7])
                                .style(crate::theme::ghost_button),
                        );
                }
                controls = controls
                    .push(Space::new().width(Length::Fill))
                    .push(
                        text(editor.find_status.as_deref().unwrap_or_default())
                            .size(11)
                            .color(crate::theme::MUTED),
                    )
                    .push(
                        button("×")
                            .on_press(Message::EditorFindClosed)
                            .padding([2, 5])
                            .style(crate::theme::ghost_button),
                    );
                container(controls)
                    .padding([4, 7])
                    .width(Length::Fill)
                    .style(crate::theme::top_bar)
            });
            let mut layout = column![editor_header(
                state,
                *dirty,
                saving.is_some(),
                *close_confirmation,
                markdown_tools,
                markdown_preview,
            )];
            if let Some(find_bar) = find_bar {
                layout = layout.push(find_bar);
            }
            if state.ui_settings().rich_markdown_spellcheck && editor.is_markdown() {
                let spelling = if editor.misspellings().is_empty() {
                    "Spelling · ✓".to_string()
                } else {
                    format!("Spelling · {}", editor.misspellings().join(", "))
                };
                layout =
                    layout.push(
                        container(text(spelling).size(11).color(
                            if editor.misspellings().is_empty() {
                                crate::theme::MUTED
                            } else {
                                Color::from_rgb8(0xd0, 0x58, 0x58)
                            },
                        ))
                        .padding([3, 9])
                        .width(Length::Fill)
                        .style(crate::theme::top_bar),
                    );
            }
            if markdown_preview {
                layout = layout.push(
                    container(scrollable(
                        markdown::view(
                            markdown.items(),
                            markdown::Settings::with_text_size(
                                15,
                                crate::theme::app_theme(&state.ui_settings().theme),
                            ),
                        )
                        .map(Message::EditorMarkdownLinkClicked),
                    ))
                    .padding([14, 18])
                    .height(Length::Fill),
                );
            } else {
                let text_editor: Element<'_, Message> = container(
                    text_editor(content)
                        .on_action(Message::EditorAction)
                        .height(Length::Fill)
                        .size(15)
                        .font(crate::editor_font::resolve(state.ui_settings()))
                        .wrapping(if state.ui_settings().editor_word_wrap {
                            Wrapping::WordOrGlyph
                        } else {
                            Wrapping::None
                        }),
                )
                .padding([8, 10])
                .height(Length::Fill)
                .into();
                let editor_body: Element<'_, Message> = if state.ui_settings().editor_minimap {
                    let minimap = container(scrollable(
                        text(content.text())
                            .size(4)
                            .font(crate::editor_font::resolve(state.ui_settings()))
                            .color(crate::theme::MUTED)
                            .wrapping(Wrapping::None),
                    ))
                    .width(Length::Fixed(88.0))
                    .height(Length::Fill)
                    .padding([7, 5])
                    .style(crate::theme::top_bar);
                    row![text_editor, minimap].height(Length::Fill).into()
                } else {
                    text_editor
                };
                layout = layout.push(editor_body);
            }
            if let Some(error) = error {
                layout = layout.push(
                    text(error)
                        .size(13)
                        .color(Color::from_rgb(0.75, 0.22, 0.17)),
                );
            }
            layout.into()
        }
    };
    Some(
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(crate::theme::editor_surface)
            .into(),
    )
}

fn editor_header(
    state: &AppState,
    dirty: bool,
    saving: bool,
    close_confirmation: bool,
    markdown_tools: bool,
    markdown_preview: bool,
) -> Element<'_, Message> {
    let editor = state.editor();
    let mut tabs = row![].spacing(2).align_y(Alignment::Center);
    if let (Some(panes), Some(worktree)) = (state.panes(), editor.worktree()) {
        for (pane, session) in panes
            .iter()
            .filter(|(_, session)| state.worktree_for_session(**session) == Some(worktree))
        {
            let terminal = button(
                row![
                    text("▣").size(12).color(crate::theme::MUTED),
                    text(state.session_tab_title(*session)).size(13),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .on_press(Message::WorkspaceTerminalTabSelected(*session))
            .padding([3, 5])
            .style(crate::theme::ghost_button);
            let close = button("×")
                .on_press(Message::PaneCloseRequested(*pane))
                .padding([2, 4])
                .style(crate::theme::ghost_button);
            tabs = tabs.push(
                container(row![terminal, close].spacing(0).align_y(Alignment::Center))
                    .style(crate::theme::top_bar),
            );
        }
    }
    for tab in editor
        .tabs()
        .into_iter()
        .filter(|tab| editor.worktree() == Some(&tab.worktree))
    {
        let name = std::path::Path::new(&tab.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&tab.path)
            .to_string();
        let marker = if tab.saving {
            "…"
        } else if tab.dirty {
            "M"
        } else {
            ""
        };
        let select = button(
            row![
                text("▤").size(12).color(crate::theme::MUTED),
                text(name).size(13),
                text(marker).size(11).color(crate::theme::MUTED),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .on_press(Message::EditorTabSelected {
            worktree: tab.worktree.clone(),
            path: tab.path.clone(),
        })
        .padding([3, 5])
        .style(if tab.active {
            crate::theme::selected_button
        } else {
            crate::theme::ghost_button
        });
        let close = button("×")
            .on_press(Message::EditorTabCloseRequested {
                worktree: tab.worktree,
                path: tab.path,
            })
            .padding([2, 4])
            .style(crate::theme::ghost_button);
        tabs = tabs.push(
            container(row![select, close].spacing(0).align_y(Alignment::Center))
                .style(crate::theme::top_bar),
        );
    }
    let tabs = scrollable(tabs)
        .direction(iced::widget::scrollable::Direction::Horizontal(
            iced::widget::scrollable::Scrollbar::new(),
        ))
        .height(Length::Fixed(28.0))
        .width(Length::Fill);
    let mut header = row![tabs];
    if markdown_tools {
        header = header.push(
            button(if markdown_preview { "Edit" } else { "Preview" })
                .on_press(Message::EditorMarkdownPreviewToggled)
                .padding([3, 7])
                .style(crate::theme::ghost_button),
        );
    }
    header = header.push(
        button("Save")
            .on_press_maybe((dirty && !saving).then_some(Message::EditorSaveRequested))
            .padding([3, 7])
            .style(crate::theme::ghost_button),
    );
    if close_confirmation {
        header = header
            .push(button("Keep editing").on_press(Message::EditorCloseCancelled))
            .push(button("Discard changes").on_press(Message::EditorDiscardConfirmed));
    }
    container(header.spacing(5).align_y(Alignment::Center))
        .height(Length::Fixed(28.0))
        .width(Length::Fill)
        .padding([2, 4])
        .style(crate::theme::top_bar)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text_editor::{Action, Edit};
    use std::time::SystemTime;

    fn signature(size: u64) -> FileSignature {
        FileSignature {
            size,
            mtime: SystemTime::UNIX_EPOCH,
            change_marker: None,
            content_hash: Some([0; 32]),
        }
    }

    #[test]
    fn external_watch_only_tracks_the_exact_clean_disk_version() {
        let (mut editor, worktree) = ready_state_at("src/lib.rs", "hello");
        let (_, _, expected) = editor.external_watch_target().unwrap();
        assert!(editor.is_clean_version(&worktree, "src/lib.rs", &expected));

        editor.perform(text_editor::Action::Edit(text_editor::Edit::Insert('!')));
        assert!(editor.external_watch_target().is_none());
        assert!(!editor.is_clean_version(&worktree, "src/lib.rs", &expected));
    }

    fn ready_state_at(path: &str, text: &str) -> (EditorState, WorktreeId) {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = EditorState::default();
        state.begin_load(worktree.clone(), path.into(), OpId(1));
        assert!(state.accept_load(
            &worktree,
            path,
            OpId(1),
            Ok(EditorLoad::Ready {
                text: text.into(),
                size: text.len() as u64,
                signature: signature(text.len() as u64),
            })
        ));
        (state, worktree)
    }

    #[test]
    fn supported_binary_images_open_as_bounded_previews() {
        let load = binary_preview("assets/logo.png", vec![0, 1, 2], 3);
        assert!(matches!(
            load,
            EditorLoad::Preview {
                kind: PreviewKind::Image,
                size: 3,
                ..
            }
        ));
        assert!(matches!(
            binary_preview("archive.bin", vec![0, 1, 2], 3),
            EditorLoad::Binary { size: 3 }
        ));
    }

    fn ready_state(text: &str) -> (EditorState, WorktreeId) {
        ready_state_at("src/lib.rs", text)
    }

    #[test]
    fn concurrent_load_completes_without_replacing_the_active_document() {
        let a = WorktreeId("/tmp/a".into());
        let b = WorktreeId("/tmp/b".into());
        let mut state = EditorState::default();
        state.begin_load(a.clone(), "a.rs".into(), OpId(1));
        state.begin_load(b.clone(), "b.rs".into(), OpId(2));

        assert!(state.accept_load(&a, "a.rs", OpId(1), Err("old failure".into())));
        assert_eq!(state.worktree(), Some(&b));
        assert!(state.activate(&a, "a.rs"));
        assert!(matches!(state.document, Document::Unavailable { .. }));
    }

    #[test]
    fn switching_tabs_preserves_dirty_buffers_and_closes_only_the_selected_tab() {
        let (mut state, worktree) = ready_state_at("src/a.rs", "a");
        state.perform(Action::Edit(Edit::Insert('!')));
        state.begin_load(worktree.clone(), "src/b.rs".into(), OpId(2));
        assert!(state.accept_load(
            &worktree,
            "src/b.rs",
            OpId(2),
            Ok(EditorLoad::Ready {
                text: "b".into(),
                size: 1,
                signature: signature(1),
            })
        ));
        assert_eq!(state.tabs().len(), 2);

        assert!(state.activate(&worktree, "src/a.rs"));
        let (text, dirty, _, _) = state.ready_snapshot().unwrap();
        assert_eq!(text, "!a");
        assert!(dirty);
        assert!(!state.request_close(), "dirty tabs require confirmation");
        state.discard_and_close();

        assert_eq!(state.tabs().len(), 1);
        assert!(state.is_document(&worktree, "src/b.rs"));
        assert_eq!(state.location(), Some((&worktree, "src/b.rs")));
    }

    #[test]
    fn save_completion_is_applied_even_after_switching_tabs() {
        let (mut state, worktree) = ready_state_at("src/a.rs", "a");
        state.perform(Action::Edit(Edit::Insert('!')));
        let save = state.begin_save(OpId(2)).unwrap();
        state.begin_load(worktree.clone(), "src/b.rs".into(), OpId(3));

        assert!(state.accept_save(
            &worktree,
            "src/a.rs",
            save.op,
            Ok(WriteOutcome::Written {
                signature: signature(2),
            })
        ));
        assert!(state.activate(&worktree, "src/a.rs"));
        let (_, dirty, saving, error) = state.ready_snapshot().unwrap();
        assert!(!dirty);
        assert!(!saving);
        assert_eq!(error, None);
    }

    #[test]
    fn typing_during_save_keeps_the_editor_dirty_after_success() {
        let (mut state, worktree) = ready_state("a");
        state.perform(Action::Edit(Edit::Insert('b')));
        let save = state.begin_save(OpId(2)).unwrap();
        assert_eq!(save.text, "ba");

        state.perform(Action::Edit(Edit::Insert('c')));
        assert!(state.accept_save(
            &worktree,
            "src/lib.rs",
            OpId(2),
            Ok(WriteOutcome::Written {
                signature: signature(2),
            })
        ));
        let (text, dirty, saving, error) = state.ready_snapshot().unwrap();
        assert_eq!(text, "bca");
        assert!(dirty, "the post-snapshot edit has not been saved");
        assert!(!saving);
        assert_eq!(error, None);
    }

    #[test]
    fn stale_disk_conflict_never_marks_the_buffer_clean() {
        let (mut state, worktree) = ready_state("a");
        state.perform(Action::Edit(Edit::Insert('b')));
        state.begin_save(OpId(2)).unwrap();

        assert!(state.accept_save(
            &worktree,
            "src/lib.rs",
            OpId(2),
            Ok(WriteOutcome::StaleConflict { disk: None })
        ));
        let (_, dirty, saving, error) = state.ready_snapshot().unwrap();
        assert!(dirty);
        assert!(!saving);
        assert!(error.unwrap().contains("changed on disk"));
    }

    #[test]
    fn dirty_document_requires_explicit_discard_before_close() {
        let (mut state, _) = ready_state("a");
        state.perform(Action::Edit(Edit::Insert('b')));

        assert!(!state.request_close());
        assert!(state.is_open());
        assert!(state.has_unsaved_changes());

        state.cancel_close();
        assert!(state.is_open());
        state.discard_and_close();
        assert!(!state.is_open());
    }

    #[test]
    fn find_wraps_and_replace_all_marks_the_document_dirty() {
        let (mut state, _) = ready_state("one fish\n two fish");
        state.open_find(true);
        state.set_find_query("fish".into());
        assert!(state.find_next(false));
        assert_eq!(state.find_match, Some((4, 8)));
        assert!(state.find_next(false));
        assert_eq!(state.find_match, Some((14, 18)));
        assert!(state.find_next(false));
        assert_eq!(state.find_match, Some((4, 8)), "find-next must wrap");

        state.set_replacement("whale".into());
        assert_eq!(state.replace_all(), 2);
        let (text, dirty, _, _) = state.ready_snapshot().unwrap();
        assert_eq!(text, "one whale\n two whale");
        assert!(dirty);
    }

    #[test]
    fn markdown_preview_is_scoped_to_markdown_and_reparsed_after_edits() {
        let (mut markdown_editor, _) = ready_state_at("README.md", "# Before");
        assert!(markdown_editor.is_markdown());
        assert!(!markdown_editor.markdown_preview());
        markdown_editor.toggle_markdown_preview();
        assert!(markdown_editor.markdown_preview());

        markdown_editor.perform(Action::Edit(Edit::Insert('!')));
        let Document::Ready {
            content, markdown, ..
        } = &markdown_editor.document
        else {
            panic!("markdown document must stay ready");
        };
        assert_eq!(content.text(), "!# Before");
        assert!(
            !markdown.items().is_empty(),
            "the preview cache must be rebuilt from the edited buffer"
        );

        let (mut rust_editor, _) = ready_state("fn main() {}");
        rust_editor.toggle_markdown_preview();
        assert!(!rust_editor.markdown_preview());
    }
}
