-- History augments immutable events; current Participant rows keep their meaning.
ALTER TABLE board_activity ADD COLUMN participant_key TEXT REFERENCES board_identities(identity_key);
ALTER TABLE board_activity ADD COLUMN participant_role TEXT;
ALTER TABLE board_activity ADD COLUMN replaced_participant_key TEXT REFERENCES board_identities(identity_key);
ALTER TABLE board_messages ADD COLUMN posted_from_activity INTEGER REFERENCES board_activity(activity_sequence);
CREATE INDEX board_activity_participant_history
  ON board_activity(root_id, participant_key, activity_sequence) WHERE participant_key IS NOT NULL;

-- Step 1: joins and leaves always identify their subject, even if Role was lost.
UPDATE board_activity SET participant_key = actor_key
WHERE kind IN ('participantJoined', 'participantLeft');

-- Step 2a: surviving leaver closure proves a handover to its replacement.
UPDATE board_activity AS activity
SET participant_key = holder.replaced_by,
    participant_role = 'orchestrator',
    replaced_participant_key = activity.actor_key
FROM thread_participants AS holder
WHERE activity.kind = 'orchestratorReplaced'
  AND holder.root_id = activity.root_id
  AND holder.reader_key = activity.actor_key
  AND holder.closed_at_activity = activity.activity_sequence
  AND holder.replaced_by IS NOT NULL;

-- Step 2b: surviving join/holder evidence proves join --replace. Kind proves Role.
UPDATE board_activity AS activity
SET participant_key = activity.actor_key,
    participant_role = CASE activity.kind
      WHEN 'orchestratorReplaced' THEN 'orchestrator' ELSE 'implementer' END,
    replaced_participant_key = (
      SELECT holder.reader_key FROM thread_participants AS holder
      WHERE holder.root_id = activity.root_id
        AND holder.closed_at_activity = activity.activity_sequence
        AND holder.replaced_by = activity.actor_key
    )
WHERE activity.kind IN ('orchestratorReplaced', 'implementerReplaced')
  AND activity.participant_key IS NULL
  AND EXISTS (
    SELECT 1 FROM thread_participants AS participant
    WHERE participant.root_id = activity.root_id
      AND ((participant.reader_key = activity.actor_key
            AND participant.joined_at_activity = activity.activity_sequence)
        OR (participant.closed_at_activity = activity.activity_sequence
            AND participant.replaced_by = activity.actor_key))
  );

-- Step 3: promotion can overwrite a join's Role without moving its sequence.
UPDATE board_activity AS activity SET participant_role = participant.role
FROM thread_participants AS participant
WHERE activity.kind = 'participantJoined'
  AND participant.root_id = activity.root_id
  AND participant.reader_key = activity.actor_key
  AND participant.joined_at_activity = activity.activity_sequence
  AND (participant.role <> 'orchestrator' OR NOT EXISTS (
    SELECT 1 FROM board_activity AS later
    WHERE later.root_id = activity.root_id
      AND later.activity_sequence > activity.activity_sequence
      AND later.kind = 'orchestratorReplaced'
      AND (later.participant_key = participant.reader_key OR later.participant_key IS NULL)
  ));

-- Step 4: select the latest Role barrier BEFORE accepting a known grant.
UPDATE board_messages AS message SET posted_from_activity = (
  SELECT CASE WHEN latest_role_barrier.participant_key = message.actor_key
                   AND latest_role_barrier.participant_role IS NOT NULL
              THEN latest_role_barrier.activity_sequence END
  FROM (
    SELECT activity_sequence, participant_key, participant_role, kind
    FROM board_activity
    WHERE root_id = message.root_id
      AND activity_sequence < (SELECT activity_sequence FROM board_activity WHERE message_id = message.message_id)
      AND (
        (kind IN ('participantJoined', 'participantLeft') AND actor_key = message.actor_key)
        OR (kind IN ('orchestratorReplaced', 'implementerReplaced')
          AND (participant_key = message.actor_key
            OR replaced_participant_key = message.actor_key
            OR participant_key IS NULL))
        OR kind = 'threadResolved'
      )
    ORDER BY activity_sequence DESC
    LIMIT 1
  ) AS latest_role_barrier
)
WHERE message.root_id IS NOT NULL
  AND (SELECT kind FROM board_identities WHERE identity_key = message.actor_key) = 'session';

-- Step 5: existing main messages stay NULL; later joins prove no creation Role.
