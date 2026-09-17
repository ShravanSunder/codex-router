CREATE UNIQUE INDEX thread_single_implementer
  ON thread_participants(root_id)
  WHERE role='implementer' AND closed_at_activity IS NULL;
