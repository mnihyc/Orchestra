-- The history indexes, applied by the scale test to its connection-local replicas of the tables.
-- A history check starts with uncertain inputs instead of loading every completed turn.
CREATE INDEX inputs_uncertain_history_idx ON thread_inputs(thread_id,id)
  WHERE state<>'delivered';
CREATE INDEX turns_input_history_idx ON turns(thread_id,input_id)
  INCLUDE(continuation_id,ordinal);
CREATE INDEX items_before_history_idx ON items(thread_id,continuation_id,seq_created)
  WHERE before_orch AND turn_id IS NULL AND stream='main';
