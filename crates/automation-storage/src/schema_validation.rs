//! Exact structural validation for the current automation migration target.
//!
//! Future migrations must update these target specifications with their schema change.
use crate::StorageError;
use sqlx::{Row, Sqlite, Transaction};

const CURRENT_TARGET_DEFINITION_SOURCE: &str = concat!(
    include_str!("../migrations/20260910000000_automation_v1.sql"),
    "\n",
    include_str!("../migrations/20260928000000_latest_agent_sender.sql"),
    "\n",
    include_str!("../migrations/20260930000000_router_pushes.sql"),
);
const ROUTER_PUSH_TABLE: &str = "router_pushes";

struct TableSpec {
    name: &'static str,
    columns: &'static str,
    foreign_keys: &'static str,
}

const TABLE_SPECS: [TableSpec; 11] = [
    TableSpec {
        name: "automation_events",
        columns: "event_sequence,INTEGER,0,<NULL>,1;event_id,TEXT,1,<NULL>,0;subject_kind,TEXT,1,<NULL>,0;subject_id,TEXT,1,<NULL>,0;event_kind,TEXT,1,<NULL>,0;event_body_json,TEXT,1,<NULL>,0;recorded_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "",
    },
    TableSpec {
        name: "instruction_documents",
        columns: "instruction_id,TEXT,0,<NULL>,1;current_revision_id,TEXT,1,<NULL>,0;instruction_text,TEXT,1,<NULL>,0;updated_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "0,0,instruction_revisions,instruction_id,instruction_id,NO ACTION,NO ACTION,NONE;0,1,instruction_revisions,current_revision_id,revision_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "instruction_revisions",
        columns: "revision_id,TEXT,0,<NULL>,1;instruction_id,TEXT,1,<NULL>,0;instruction_text,TEXT,1,<NULL>,0;source_revision_id,TEXT,0,<NULL>,0;recorded_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "0,0,instruction_documents,instruction_id,instruction_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "mailbox_deliveries",
        columns: "delivery_id,TEXT,0,<NULL>,1;wakeup_id,TEXT,1,<NULL>,0;occurrence_id,TEXT,1,<NULL>,0;due_at_ms,INTEGER,1,<NULL>,0;fired_at_ms,INTEGER,1,<NULL>,0;target_json,TEXT,1,<NULL>,0;message_json,TEXT,1,<NULL>,0;delivery_mode,TEXT,1,<NULL>,0;generation_guard_json,TEXT,0,<NULL>,0;delivery_status,TEXT,1,<NULL>,0;eligible_at_ms,INTEGER,1,<NULL>,0;expires_at_ms,INTEGER,0,<NULL>,0;latest_attempt_json,TEXT,0,<NULL>,0;accepted_receipt_json,TEXT,0,<NULL>,0;created_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "0,0,wakeup_definitions,wakeup_id,wakeup_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "operation_receipts",
        columns: "operation_id,TEXT,0,<NULL>,1;method_name,TEXT,1,<NULL>,0;canonical_request,BLOB,1,<NULL>,0;resource_id,TEXT,1,<NULL>,0;operation_status,TEXT,1,<NULL>,0;effect_evidence_json,TEXT,1,<NULL>,0;final_result_json,TEXT,0,<NULL>,0;final_error_json,TEXT,0,<NULL>,0;committed_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "",
    },
    TableSpec {
        name: "router_pushes",
        columns: "push_id,TEXT,1,<NULL>,1;kind,TEXT,1,<NULL>,0;origin_kind,TEXT,1,<NULL>,0;origin_service_id,TEXT,0,<NULL>,0;origin_endpoint_id,TEXT,0,<NULL>,0;origin_session_id,TEXT,0,<NULL>,0;origin_router_ref,TEXT,0,<NULL>,0;target_service_id,TEXT,1,<NULL>,0;target_endpoint_id,TEXT,1,<NULL>,0;target_session_id,TEXT,1,<NULL>,0;reply_to_push_id,TEXT,0,<NULL>,0;header_facts_json,TEXT,1,<NULL>,0;body,TEXT,0,<NULL>,0;ranges_json,TEXT,0,<NULL>,0;delivery_state,TEXT,1,<NULL>,0;last_outcome_json,TEXT,0,<NULL>,0;created_at,TEXT,1,<NULL>,0;settled_at,TEXT,0,<NULL>,0;read_at,TEXT,0,<NULL>,0",
        foreign_keys: "0,0,router_pushes,reply_to_push_id,push_id,NO ACTION,SET NULL,NONE",
    },
    TableSpec {
        name: "schedule_definitions",
        columns: "schedule_id,TEXT,0,<NULL>,1;change_id,TEXT,1,<NULL>,0;instruction_id,TEXT,1,<NULL>,0;enabled,INTEGER,1,<NULL>,0;definition_json,TEXT,1,<NULL>,0;imported_continuity_json,TEXT,1,<NULL>,0;created_at_ms,INTEGER,1,<NULL>,0;updated_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "0,0,instruction_documents,instruction_id,instruction_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "schedule_timing_state",
        columns: "schedule_id,TEXT,0,<NULL>,1;applied_change_id,TEXT,1,<NULL>,0;anchor_at_ms,INTEGER,1,<NULL>,0;evaluated_through_ms,INTEGER,1,<NULL>,0;next_due_at_ms,INTEGER,0,<NULL>,0",
        foreign_keys: "0,0,schedule_definitions,schedule_id,schedule_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "thread_bindings",
        columns: "thread_binding_id,TEXT,0,<NULL>,1;schedule_id,TEXT,1,<NULL>,0;service_id,TEXT,1,<NULL>,0;endpoint_id,TEXT,1,<NULL>,0;thread_id,TEXT,1,<NULL>,0;claimed_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "0,0,schedule_definitions,schedule_id,schedule_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "wakeup_definitions",
        columns: "wakeup_id,TEXT,0,<NULL>,1;change_id,TEXT,1,<NULL>,0;wakeup_status,TEXT,1,<NULL>,0;definition_json,TEXT,1,<NULL>,0;anchor_at_ms,INTEGER,1,<NULL>,0;evaluated_through_ms,INTEGER,1,<NULL>,0;next_due_at_ms,INTEGER,0,<NULL>,0;expires_at_ms,INTEGER,0,<NULL>,0;first_fire_json,TEXT,0,<NULL>,0;pending_delivery_id,TEXT,0,<NULL>,0;created_at_ms,INTEGER,1,<NULL>,0;updated_at_ms,INTEGER,1,<NULL>,0",
        foreign_keys: "0,0,mailbox_deliveries,wakeup_id,wakeup_id,NO ACTION,NO ACTION,NONE;0,1,mailbox_deliveries,pending_delivery_id,delivery_id,NO ACTION,NO ACTION,NONE",
    },
    TableSpec {
        name: "workflow_runs",
        columns: "run_id,TEXT,0,<NULL>,1;schedule_id,TEXT,1,<NULL>,0;due_at_ms,INTEGER,1,<NULL>,0;run_status,TEXT,1,<NULL>,0;captured_inputs_json,TEXT,0,<NULL>,0;thread_binding_id,TEXT,0,<NULL>,0;native_turn_id,TEXT,0,<NULL>,0;execution_started_at_ms,INTEGER,0,<NULL>,0;execution_deadline_at_ms,INTEGER,0,<NULL>,0;effective_timeout_seconds,INTEGER,0,<NULL>,0;execution_evidence_json,TEXT,1,<NULL>,0;worker_outcome_json,TEXT,0,<NULL>,0;summary_attempt_json,TEXT,0,<NULL>,0;summary_text,TEXT,0,<NULL>,0;summary_source_json,TEXT,0,<NULL>,0;completed_at_ms,INTEGER,0,<NULL>,0",
        foreign_keys: "0,0,thread_bindings,schedule_id,schedule_id,NO ACTION,NO ACTION,NONE;0,1,thread_bindings,thread_binding_id,thread_binding_id,NO ACTION,NO ACTION,NONE;1,0,schedule_definitions,schedule_id,schedule_id,NO ACTION,NO ACTION,NONE",
    },
];

const INDEX_SPECS: [&str; 26] = [
    "automation_events,event_cleanup,0,c,0,recorded_at_ms:0:BINARY:1,event_sequence:0:BINARY:1",
    "automation_events,event_history,0,c,0,subject_kind:0:BINARY:1,subject_id:0:BINARY:1,event_sequence:0:BINARY:1",
    "automation_events,_,1,u,0,event_id:0:BINARY:1",
    "instruction_documents,_,1,pk,0,instruction_id:0:BINARY:1",
    "instruction_revisions,_,1,u,0,instruction_id:0:BINARY:1,revision_id:0:BINARY:1",
    "instruction_revisions,_,1,pk,0,revision_id:0:BINARY:1",
    "mailbox_deliveries,delivery_eligibility,0,c,0,delivery_status:0:BINARY:1,eligible_at_ms:0:BINARY:1",
    "mailbox_deliveries,_,1,u,0,wakeup_id:0:BINARY:1,delivery_id:0:BINARY:1",
    "mailbox_deliveries,_,1,u,0,occurrence_id:0:BINARY:1",
    "mailbox_deliveries,_,1,pk,0,delivery_id:0:BINARY:1",
    "operation_receipts,_,1,pk,0,operation_id:0:BINARY:1",
    "router_pushes,router_pushes_created,0,c,0,created_at:0:BINARY:1",
    "router_pushes,router_pushes_origin_ref,1,c,0,origin_kind:0:BINARY:1,origin_router_ref:0:BINARY:1",
    "router_pushes,router_pushes_target_state,0,c,0,target_service_id:0:BINARY:1,target_endpoint_id:0:BINARY:1,target_session_id:0:BINARY:1,delivery_state:0:BINARY:1,created_at:0:BINARY:1",
    "router_pushes,_,1,pk,0,push_id:0:BINARY:1",
    "schedule_definitions,_,1,pk,0,schedule_id:0:BINARY:1",
    "schedule_timing_state,_,1,pk,0,schedule_id:0:BINARY:1",
    "thread_bindings,_,1,u,0,schedule_id:0:BINARY:1,thread_binding_id:0:BINARY:1",
    "thread_bindings,_,1,u,0,service_id:0:BINARY:1,endpoint_id:0:BINARY:1,thread_id:0:BINARY:1",
    "thread_bindings,_,1,pk,0,thread_binding_id:0:BINARY:1",
    "wakeup_definitions,_,1,u,0,pending_delivery_id:0:BINARY:1",
    "wakeup_definitions,_,1,pk,0,wakeup_id:0:BINARY:1",
    "workflow_runs,run_history,0,c,0,schedule_id:0:BINARY:1,due_at_ms:0:BINARY:1,run_id:0:BINARY:1",
    "workflow_runs,run_admission_lookup,0,c,0,schedule_id:0:BINARY:1,run_status:0:BINARY:1",
    "workflow_runs,_,1,u,0,schedule_id:0:BINARY:1,run_id:0:BINARY:1",
    "workflow_runs,_,1,pk,0,run_id:0:BINARY:1",
];

pub(crate) async fn has_domain_objects(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, StorageError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
    )
    .fetch_one(&mut **transaction)
    .await?;
    Ok(count != 0)
}

pub(crate) async fn validate_target_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), StorageError> {
    validate_schema(transaction, false).await
}

pub(crate) async fn validate_legacy_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), StorageError> {
    validate_schema(transaction, true).await
}

async fn validate_schema(
    transaction: &mut Transaction<'_, Sqlite>,
    legacy: bool,
) -> Result<(), StorageError> {
    validate_object_inventory(transaction, legacy).await?;
    for table in TABLE_SPECS
        .iter()
        .filter(|table| !legacy || table.name != ROUTER_PUSH_TABLE)
    {
        validate_columns(transaction, table, legacy).await?;
        validate_foreign_keys(transaction, table).await?;
    }
    validate_indexes(transaction, legacy).await?;
    validate_known_definitions(transaction, legacy).await?;
    Ok(())
}

async fn validate_object_inventory(
    transaction: &mut Transaction<'_, Sqlite>,
    legacy: bool,
) -> Result<(), StorageError> {
    let rows = sqlx::query(
        "SELECT type,name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY type,name",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let mut actual_tables = Vec::new();
    let mut actual_named_indexes = Vec::new();
    for row in rows {
        let object_type: String = row.get("type");
        let name: String = row.get("name");
        match object_type.as_str() {
            "table" => actual_tables.push(name),
            "index" => actual_named_indexes.push(name),
            _ => return Err(StorageError::InvalidSchema),
        }
    }
    let expected_tables: Vec<&str> = TABLE_SPECS
        .iter()
        .filter(|table| !legacy || table.name != ROUTER_PUSH_TABLE)
        .map(|table| table.name)
        .collect();
    let expected_indexes = if legacy {
        vec![
            "delivery_eligibility",
            "event_cleanup",
            "event_history",
            "run_admission_lookup",
            "run_history",
        ]
    } else {
        vec![
            "delivery_eligibility",
            "event_cleanup",
            "event_history",
            "router_pushes_created",
            "router_pushes_origin_ref",
            "router_pushes_target_state",
            "run_admission_lookup",
            "run_history",
        ]
    };
    if actual_tables != expected_tables || actual_named_indexes != expected_indexes {
        return Err(StorageError::InvalidSchema);
    }
    Ok(())
}

async fn validate_columns(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &TableSpec,
    legacy: bool,
) -> Result<(), StorageError> {
    let rows = sqlx::query(
        "SELECT name,type,\"notnull\",coalesce(dflt_value,'<NULL>') AS default_value,pk FROM pragma_table_info(?1) ORDER BY cid",
    )
    .bind(table.name)
    .fetch_all(&mut **transaction)
    .await?;
    let actual = rows
        .into_iter()
        .map(|row| {
            format!(
                "{},{},{},{},{}",
                row.get::<String, _>("name"),
                row.get::<String, _>("type"),
                row.get::<i64, _>("notnull"),
                row.get::<String, _>("default_value"),
                row.get::<i64, _>("pk")
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let expected = if !legacy && table.name == "mailbox_deliveries" {
        table
            .columns
            .replace("accepted_receipt_json", "outcome_receipt_json")
    } else {
        table.columns.to_owned()
    };
    if actual != expected {
        return Err(StorageError::InvalidSchema);
    }
    Ok(())
}

async fn validate_foreign_keys(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &TableSpec,
) -> Result<(), StorageError> {
    let rows = sqlx::query(
        "SELECT id,seq,\"table\",\"from\",\"to\",on_update,on_delete,match FROM pragma_foreign_key_list(?1) ORDER BY id,seq",
    )
    .bind(table.name)
    .fetch_all(&mut **transaction)
    .await?;
    let actual = rows
        .into_iter()
        .map(|row| {
            format!(
                "{},{},{},{},{},{},{},{}",
                row.get::<i64, _>("id"),
                row.get::<i64, _>("seq"),
                row.get::<String, _>("table"),
                row.get::<String, _>("from"),
                row.get::<String, _>("to"),
                row.get::<String, _>("on_update"),
                row.get::<String, _>("on_delete"),
                row.get::<String, _>("match")
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    if actual != table.foreign_keys {
        return Err(StorageError::InvalidSchema);
    }
    Ok(())
}

async fn validate_indexes(
    transaction: &mut Transaction<'_, Sqlite>,
    legacy: bool,
) -> Result<(), StorageError> {
    let mut actual = Vec::new();
    for table in TABLE_SPECS
        .iter()
        .filter(|table| !legacy || table.name != ROUTER_PUSH_TABLE)
    {
        let indexes = sqlx::query(
            "SELECT name,\"unique\",origin,partial FROM pragma_index_list(?1) ORDER BY seq",
        )
        .bind(table.name)
        .fetch_all(&mut **transaction)
        .await?;
        for index in indexes {
            let name: String = index.get("name");
            let origin: String = index.get("origin");
            let columns = sqlx::query(
                "SELECT name,desc,coll,key FROM pragma_index_xinfo(?1) WHERE key=1 ORDER BY seqno",
            )
            .bind(&name)
            .fetch_all(&mut **transaction)
            .await?
            .into_iter()
            .map(|column| {
                format!(
                    "{}:{}:{}:{}",
                    column.get::<String, _>("name"),
                    column.get::<i64, _>("desc"),
                    column.get::<String, _>("coll"),
                    column.get::<i64, _>("key")
                )
            })
            .collect::<Vec<_>>();
            actual.push(format!(
                "{},{},{},{},{},{}",
                table.name,
                if origin == "c" { name.as_str() } else { "_" },
                index.get::<i64, _>("unique"),
                origin,
                index.get::<i64, _>("partial"),
                columns.join(",")
            ));
        }
    }
    actual.sort();
    let mut expected = INDEX_SPECS
        .iter()
        .filter(|index| !legacy || !index.starts_with("router_pushes,"))
        .copied()
        .collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        return Err(StorageError::InvalidSchema);
    }
    Ok(())
}

async fn validate_known_definitions(
    transaction: &mut Transaction<'_, Sqlite>,
    legacy: bool,
) -> Result<(), StorageError> {
    for table in TABLE_SPECS
        .iter()
        .filter(|table| !legacy || table.name != ROUTER_PUSH_TABLE)
    {
        let sql: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='table' AND name=?1")
                .bind(table.name)
                .fetch_one(&mut **transaction)
                .await?;
        if tokenize_schema_definition(&sql)?
            != expected_definition_tokens("table", table.name, legacy)?
        {
            return Err(StorageError::InvalidSchema);
        }
    }
    for index in INDEX_SPECS
        .iter()
        .filter(|index| !legacy || !index.starts_with("router_pushes,"))
        .filter_map(|index| {
            let mut fields = index.split(',');
            fields.next();
            let name = fields.next()?;
            (name != "_").then_some(name)
        })
    {
        let sql: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='index' AND name=?1")
                .bind(index)
                .fetch_one(&mut **transaction)
                .await?;
        if tokenize_schema_definition(&sql)? != expected_definition_tokens("index", index, legacy)?
        {
            return Err(StorageError::InvalidSchema);
        }
    }
    Ok(())
}

fn expected_definition_tokens(
    object_type: &str,
    object_name: &str,
    legacy: bool,
) -> Result<Vec<String>, StorageError> {
    let source = if legacy {
        CURRENT_TARGET_DEFINITION_SOURCE.to_owned()
    } else {
        CURRENT_TARGET_DEFINITION_SOURCE.replace("accepted_receipt_json", "outcome_receipt_json")
    };
    let tokens = tokenize_schema_definition(&source)?;
    let index_is_unique = if object_type == "index" {
        index_is_declared_unique(object_name)?
    } else {
        false
    };
    tokens
        .split(|token| token == ";")
        .find(|statement| {
            definition_prefix_matches(statement, object_type, object_name, index_is_unique)
        })
        .map(<[String]>::to_vec)
        .ok_or(StorageError::InvalidSchema)
}

fn index_is_declared_unique(index_name: &str) -> Result<bool, StorageError> {
    INDEX_SPECS
        .iter()
        .find_map(|index| {
            let mut fields = index.split(',');
            fields.next()?;
            let name = fields.next()?;
            let unique = fields.next()?;
            (name == index_name).then_some(unique == "1")
        })
        .ok_or(StorageError::InvalidSchema)
}

fn definition_prefix_matches(
    statement: &[String],
    object_type: &str,
    object_name: &str,
    index_is_unique: bool,
) -> bool {
    let matches_prefix = |prefix: &[&str]| {
        statement
            .get(..prefix.len())
            .is_some_and(|tokens| tokens.iter().map(String::as_str).eq(prefix.iter().copied()))
    };
    match (object_type, index_is_unique) {
        ("index", true) => matches_prefix(&["create", "unique", "index", object_name]),
        ("index", false) => matches_prefix(&["create", "index", object_name]),
        ("table", _) => matches_prefix(&["create", "table", object_name]),
        _ => false,
    }
}

#[cfg(test)]
fn contains_token_sequence(tokens: &[String], expected: &[&str]) -> bool {
    tokens.windows(expected.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    })
}

fn tokenize_schema_definition(sql: &str) -> Result<Vec<String>, StorageError> {
    let mut tokens = Vec::new();
    let mut characters = sql.chars().peekable();
    while let Some(character) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        if character == '-' && characters.peek() == Some(&'-') {
            characters.next();
            for comment_character in characters.by_ref() {
                if comment_character == '\n' {
                    break;
                }
            }
            continue;
        }
        if character == '/' && characters.peek() == Some(&'*') {
            characters.next();
            let mut previous = '\0';
            loop {
                let Some(comment_character) = characters.next() else {
                    return Err(StorageError::InvalidSchema);
                };
                if previous == '*' && comment_character == '/' {
                    break;
                }
                previous = comment_character;
            }
            continue;
        }
        if character == '\'' {
            let mut literal = String::from("'");
            loop {
                let Some(literal_character) = characters.next() else {
                    return Err(StorageError::InvalidSchema);
                };
                literal.push(literal_character);
                if literal_character == '\'' {
                    if characters.peek() == Some(&'\'') {
                        let Some(escaped_quote) = characters.next() else {
                            return Err(StorageError::InvalidSchema);
                        };
                        literal.push(escaped_quote);
                    } else {
                        break;
                    }
                }
            }
            tokens.push(literal);
            continue;
        }
        if matches!(character, '"' | '`' | '[') {
            let terminator = if character == '[' { ']' } else { character };
            let mut identifier = String::new();
            loop {
                let Some(identifier_character) = characters.next() else {
                    return Err(StorageError::InvalidSchema);
                };
                if identifier_character == terminator {
                    if characters.peek() == Some(&terminator) && terminator != ']' {
                        let Some(escaped_terminator) = characters.next() else {
                            return Err(StorageError::InvalidSchema);
                        };
                        identifier.push(escaped_terminator);
                    } else {
                        break;
                    }
                } else {
                    identifier.push(identifier_character);
                }
            }
            tokens.push(identifier.to_ascii_lowercase());
            continue;
        }
        if character.is_ascii_alphanumeric() || character == '_' {
            let mut word = String::from(character);
            while let Some(word_character) =
                characters.next_if(|next| next.is_ascii_alphanumeric() || *next == '_')
            {
                word.push(word_character);
            }
            tokens.push(word.to_ascii_lowercase());
            continue;
        }
        tokens.push(character.to_string());
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_tokenizer_preserves_literal_contents_and_token_boundaries() {
        let tokens = tokenize_schema_definition(
            "CHECK (value = 'Case Sensitive' AND quoted = \"Value Name\") -- ignored",
        )
        .expect("known schema should tokenize");
        assert!(tokens.contains(&"'Case Sensitive'".to_owned()));
        assert!(contains_token_sequence(
            &tokens,
            &["check", "(", "value", "=", "'Case Sensitive'"]
        ));
        assert_ne!(
            tokens,
            tokenize_schema_definition("CHECK(value='casesensitive')").unwrap()
        );
        assert!(tokens.contains(&"value name".to_owned()));
    }

    #[test]
    fn non_unique_same_name_index_does_not_match_unique_index_spec() {
        let is_unique =
            index_is_declared_unique("router_pushes_origin_ref").expect("declared index");
        assert!(is_unique);
        let non_unique_definition = tokenize_schema_definition(
            "CREATE INDEX router_pushes_origin_ref ON router_pushes(origin_kind, origin_router_ref)",
        )
        .expect("non-unique index definition should tokenize");
        assert!(!definition_prefix_matches(
            &non_unique_definition,
            "index",
            "router_pushes_origin_ref",
            is_unique,
        ));
        let unique_definition = tokenize_schema_definition(
            "CREATE UNIQUE INDEX router_pushes_origin_ref ON router_pushes(origin_kind, origin_router_ref)",
        )
        .expect("unique index definition should tokenize");
        assert!(definition_prefix_matches(
            &unique_definition,
            "index",
            "router_pushes_origin_ref",
            is_unique,
        ));
    }
}
