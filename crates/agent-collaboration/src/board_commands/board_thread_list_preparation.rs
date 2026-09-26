use super::board_arguments::ThreadListArguments;
use super::board_preparation::PreparedBoardCommand;
use super::board_value_parsing::{parse_identity, parse_uuid_v7, prepare_page_request};
use collaboration_client::BoardRepositoryLocation;
use collaboration_client::board::*;
use std::path::Path;

pub(super) fn prepare_thread_list(
    arguments: &ThreadListArguments,
) -> Result<PreparedBoardCommand, String> {
    let working_directory = std::env::current_dir()
        .map_err(|error| format!("could not read current working directory: {error}"))?;
    prepare_thread_list_at(arguments, &working_directory)
}

fn prepare_thread_list_at(
    arguments: &ThreadListArguments,
    working_directory: &Path,
) -> Result<PreparedBoardCommand, String> {
    match (&arguments.project_id, &arguments.repository_path) {
        (Some(project_id), None) => Ok(PreparedBoardCommand::ThreadList(ThreadListRequest {
            project_id: parse_uuid_v7(project_id.clone(), "--project-id")?,
            reader: parse_identity(
                arguments
                    .reader
                    .as_deref()
                    .ok_or("--reader is required with --project-id")?,
                "--reader",
            )?,
            watched_only: arguments.watched_only,
            page: prepare_page(arguments)?,
        })),
        (None, Some(path)) if !arguments.watched_only => {
            Ok(PreparedBoardCommand::RepositoryThreadList {
                repository: BoardRepositoryLocation::discover(path)
                    .map_err(|error| error.to_string())?,
                reader: arguments
                    .reader
                    .as_deref()
                    .map(|value| parse_identity(value, "--reader"))
                    .transpose()?,
                page: prepare_page(arguments)?,
                default_repository_path: None,
            })
        }
        (None, None) if !arguments.watched_only => Ok(PreparedBoardCommand::RepositoryThreadList {
            repository: BoardRepositoryLocation::discover(working_directory)
                .map_err(|_| thread_list_selector_error())?,
            reader: arguments
                .reader
                .as_deref()
                .map(|value| parse_identity(value, "--reader"))
                .transpose()?,
            page: prepare_page(arguments)?,
            default_repository_path: Some(working_directory.display().to_string()),
        }),
        _ => Err(thread_list_selector_error()),
    }
}

fn prepare_page(arguments: &ThreadListArguments) -> Result<PageRequest, String> {
    prepare_page_request(super::board_arguments::PageArguments {
        limit: arguments.page.limit,
        cursor: arguments.page.cursor.clone(),
    })
}

fn thread_list_selector_error() -> String {
    "Choose exactly one of --project-id or --repository-path. Example: agent-collaboration board thread list --repository-path '<path-in-repository>' (or --project-id <project-id>); --watched-only requires --project-id".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_current_repository_when_no_selector_is_given() {
        let working_directory = std::env::current_dir().expect("current directory");
        let arguments = thread_list_arguments();
        let command = prepare_thread_list_at(&arguments, &working_directory)
            .expect("current repository selection");
        assert!(matches!(
            command,
            PreparedBoardCommand::RepositoryThreadList { .. }
        ));
    }

    #[test]
    fn outside_a_repository_reports_a_corrected_selector_example() {
        let working_directory = std::env::temp_dir();
        let arguments = thread_list_arguments();
        let error = match prepare_thread_list_at(&arguments, &working_directory) {
            Err(error) => error,
            Ok(_) => panic!("a temp directory must not select a repository"),
        };
        assert!(error.contains("agent-collaboration board thread list"));
        assert!(error.contains("--repository-path '<path-in-repository>'"));
        assert!(error.contains("--project-id <project-id>"));
    }

    fn thread_list_arguments() -> ThreadListArguments {
        ThreadListArguments {
            project_id: None,
            repository_path: None,
            reader: None,
            watched_only: false,
            page: super::super::board_arguments::PageArguments {
                limit: 50,
                cursor: None,
            },
            common: super::super::board_arguments::CommonArguments {
                service_directory: None,
                json: true,
            },
        }
    }
}
