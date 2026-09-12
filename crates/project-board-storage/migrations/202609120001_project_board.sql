-- Initial board schema. Evolving value rules are validated in Rust.
CREATE TABLE board_projects (
 project_id TEXT PRIMARY KEY NOT NULL,
 name TEXT NOT NULL,
 description TEXT NOT NULL
) STRICT;
CREATE TABLE project_repositories (
 project_id TEXT NOT NULL REFERENCES board_projects(project_id),
 repository_key TEXT NOT NULL, kind TEXT NOT NULL,
 origin TEXT, service_id TEXT, common_directory TEXT,
 PRIMARY KEY(project_id, repository_key)
) STRICT;
CREATE TABLE project_boards (
 board_id TEXT PRIMARY KEY NOT NULL,
 project_id TEXT NOT NULL REFERENCES board_projects(project_id),
 name TEXT NOT NULL,
 description TEXT NOT NULL,
 state TEXT NOT NULL
) STRICT;
CREATE TABLE board_topics (
 topic_id TEXT PRIMARY KEY NOT NULL,
 board_id TEXT NOT NULL REFERENCES project_boards(board_id),
 name TEXT NOT NULL,
 description TEXT NOT NULL
) STRICT;
CREATE TABLE board_identities (
 identity_key TEXT PRIMARY KEY NOT NULL, kind TEXT NOT NULL,
 service_id TEXT, endpoint_id TEXT, session_id TEXT, human_id TEXT
) STRICT;
CREATE UNIQUE INDEX identity_session_unique
 ON board_identities(service_id,endpoint_id,session_id);
CREATE UNIQUE INDEX identity_human_unique
 ON board_identities(human_id);
CREATE TABLE board_messages (
 message_id TEXT PRIMARY KEY NOT NULL,
 topic_id TEXT NOT NULL, board_id TEXT NOT NULL, root_id TEXT,
 actor_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 acting_for_key TEXT REFERENCES board_identities(identity_key),
 text TEXT NOT NULL,
 FOREIGN KEY(topic_id,board_id) REFERENCES board_topics(topic_id,board_id),
 FOREIGN KEY(root_id) REFERENCES board_threads(root_id)
) STRICT;
CREATE TABLE board_threads (
 root_id TEXT PRIMARY KEY NOT NULL REFERENCES board_messages(message_id),
 state TEXT NOT NULL
) STRICT;
CREATE TABLE message_references (
 source_id TEXT NOT NULL REFERENCES board_messages(message_id),
 ordinal INTEGER NOT NULL, kind TEXT NOT NULL,
 target_message_id TEXT REFERENCES board_messages(message_id),
 target_root_id TEXT REFERENCES board_threads(root_id),
 PRIMARY KEY(source_id,ordinal)
) STRICT;
CREATE UNIQUE INDEX reference_message_unique ON message_references(source_id,target_message_id)
;
CREATE UNIQUE INDEX reference_thread_unique ON message_references(source_id,target_root_id)
;
CREATE TABLE activity_checkpoint (
 singleton INTEGER PRIMARY KEY,
 last_sequence INTEGER NOT NULL,
 cursor_key BLOB NOT NULL
) STRICT;
CREATE TABLE board_activity (
 activity_sequence INTEGER PRIMARY KEY,
 project_id TEXT NOT NULL, board_id TEXT NOT NULL, topic_id TEXT NOT NULL,
 root_id TEXT, kind TEXT NOT NULL,
 actor_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 message_id TEXT,
 FOREIGN KEY(board_id,project_id) REFERENCES project_boards(board_id,project_id),
 FOREIGN KEY(topic_id,board_id) REFERENCES board_topics(topic_id,board_id),
 FOREIGN KEY(message_id,topic_id)
   REFERENCES board_messages(message_id,topic_id),
 FOREIGN KEY(root_id) REFERENCES board_threads(root_id)
) STRICT;
CREATE TABLE thread_watches (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 root_id TEXT NOT NULL REFERENCES board_threads(root_id),
 active INTEGER NOT NULL CHECK(active IN (0,1)),
 starts_after_activity INTEGER NOT NULL,
 PRIMARY KEY(reader_key,root_id)
) STRICT;
CREATE TABLE project_reader_state (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 project_id TEXT NOT NULL REFERENCES board_projects(project_id),
 main_start INTEGER,
 has_unread INTEGER NOT NULL CHECK(has_unread IN (0,1)),
 PRIMARY KEY(reader_key,project_id)
) STRICT;
CREATE TABLE topic_read_bookmarks (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 topic_id TEXT NOT NULL REFERENCES board_topics(topic_id),
 through_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
 PRIMARY KEY(reader_key,topic_id)
) STRICT;
CREATE TABLE thread_read_bookmarks (
 reader_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 root_id TEXT NOT NULL REFERENCES board_threads(root_id),
 through_activity INTEGER NOT NULL REFERENCES board_activity(activity_sequence),
 PRIMARY KEY(reader_key,root_id)
) STRICT;
CREATE TABLE actor_board_cooldowns (
 actor_key TEXT NOT NULL REFERENCES board_identities(identity_key),
 board_id TEXT NOT NULL REFERENCES project_boards(board_id),
 last_post_at_ms INTEGER NOT NULL,
 PRIMARY KEY(actor_key,board_id)
) STRICT;

CREATE UNIQUE INDEX board_projects_name_unique ON board_projects(name);
CREATE UNIQUE INDEX project_boards_key_1 ON project_boards(project_id,name);
CREATE UNIQUE INDEX project_boards_key_2 ON project_boards(board_id,project_id);
CREATE UNIQUE INDEX board_topics_key_1 ON board_topics(board_id,name);
CREATE UNIQUE INDEX board_topics_key_2 ON board_topics(topic_id,board_id);
CREATE UNIQUE INDEX board_messages_key_1 ON board_messages(message_id,topic_id);
CREATE UNIQUE INDEX board_activity_message_id_unique ON board_activity(message_id);

CREATE INDEX repositories_inverse ON project_repositories(repository_key,project_id);
CREATE INDEX boards_project_state ON project_boards(project_id,state,board_id);
CREATE INDEX activity_project_order ON board_activity(project_id,activity_sequence);
CREATE INDEX activity_board_order ON board_activity(board_id,activity_sequence);
CREATE INDEX activity_topic_main ON board_activity(topic_id,activity_sequence) WHERE root_id IS NULL;
CREATE INDEX activity_thread_order ON board_activity(root_id,activity_sequence) WHERE root_id IS NOT NULL;
CREATE INDEX watches_active_readers ON thread_watches(root_id,reader_key) WHERE active=1;
CREATE INDEX summary_reader_unread ON project_reader_state(reader_key,has_unread,project_id);



INSERT INTO activity_checkpoint(singleton,last_sequence,cursor_key) VALUES(1,0,randomblob(32));
