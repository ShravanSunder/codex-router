//! Shared session-search semantics for stored discovery and interactive filtering.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSearchExpression {
    terms: Vec<SessionSearchTerm>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SessionSearchTerm {
    Bare(String),
    SessionId(String),
    Branch(String),
    Repository(String),
}

pub struct SessionSearchDocument<'a> {
    pub session_id: &'a str,
    pub name: &'a str,
    pub title: &'a str,
    pub preview: &'a str,
    pub first_user_message: &'a str,
    pub branch: &'a str,
    pub origin: &'a str,
    pub cwd: &'a str,
}

impl SessionSearchExpression {
    pub fn parse(input: &str) -> Self {
        let terms = tokenize_session_search(input)
            .into_iter()
            .map(|token| {
                let normalized = token.to_lowercase();
                if let Some(value) = normalized.strip_prefix("id:") {
                    Self::term_with_value(SessionSearchTerm::SessionId, value)
                } else if let Some(value) = normalized.strip_prefix("b:") {
                    Self::term_with_value(SessionSearchTerm::Branch, value)
                } else if let Some(value) = normalized.strip_prefix("branch:") {
                    Self::term_with_value(SessionSearchTerm::Branch, value)
                } else if let Some(value) = normalized.strip_prefix("repo:") {
                    Self::term_with_value(SessionSearchTerm::Repository, value)
                } else {
                    SessionSearchTerm::Bare(normalized)
                }
            })
            .collect();
        Self { terms }
    }

    fn term_with_value(
        constructor: impl FnOnce(String) -> SessionSearchTerm,
        value: &str,
    ) -> SessionSearchTerm {
        constructor(value.to_owned())
    }

    pub fn matches(&self, document: &SessionSearchDocument<'_>) -> bool {
        let session_id = document.session_id.to_lowercase();
        let name = document.name.to_lowercase();
        let title = document.title.to_lowercase();
        let preview = document.preview.to_lowercase();
        let first_user_message = document.first_user_message.to_lowercase();
        let branch = document.branch.to_lowercase();
        let origin = document.origin.to_lowercase();
        let cwd = document.cwd.to_lowercase();

        self.terms.iter().all(|term| match term {
            SessionSearchTerm::Bare(value) => {
                !value.is_empty()
                    && [
                        session_id.as_str(),
                        name.as_str(),
                        title.as_str(),
                        preview.as_str(),
                        first_user_message.as_str(),
                        origin.as_str(),
                        cwd.as_str(),
                    ]
                    .iter()
                    .any(|field| field.contains(value))
            }
            SessionSearchTerm::SessionId(value) => !value.is_empty() && session_id.contains(value),
            SessionSearchTerm::Branch(value) => !value.is_empty() && branch.contains(value),
            SessionSearchTerm::Repository(value) => {
                !value.is_empty() && (origin.contains(value) || cwd.contains(value))
            }
        })
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

fn tokenize_session_search(input: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in input.trim().chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}
