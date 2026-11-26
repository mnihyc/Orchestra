-- A thread, an Ask answer and a new thread's environment can be given by a pool: the accepted choice stays with what used it.
-- Retain the accepted model target on the segment that actually used it.
CREATE TABLE segment_pool_bindings (
    segment_id uuid PRIMARY KEY REFERENCES segments(id) ON DELETE CASCADE,
    choice_id uuid NOT NULL REFERENCES pool_choices(id) ON DELETE RESTRICT
);
-- Tool steps and replay keep the accepted answer's choice, including its lending path.
CREATE TABLE ask_turn_pool_bindings (
    turn_id uuid PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
    choice_id uuid UNIQUE NOT NULL REFERENCES pool_choices(id) ON DELETE RESTRICT
);
-- A template and host chosen at thread creation belong to its one provision.
CREATE TABLE environment_pool_creations (
    choice_id uuid PRIMARY KEY REFERENCES pool_choices(id) ON DELETE RESTRICT,
    environment_id uuid NOT NULL UNIQUE REFERENCES environments(id) ON DELETE CASCADE,
    job_id uuid NOT NULL UNIQUE,
    configuration jsonb NOT NULL
);
-- A place's model and provider defaults name a target, as a pool can stand where one model or provider did: a model
-- written by its routing name is that offering on the place's provider, and a provider by its id is that one provider.
UPDATE settings SET value = jsonb_build_object('type', 'offering', 'name', value #>> '{}'), revision = revision + 1
 WHERE key = 'agent.model' AND jsonb_typeof(value) = 'string';
UPDATE settings SET value = jsonb_build_object('type', 'single', 'resource', jsonb_build_object('kind', 'provider', 'id', value #>> '{}')),
       revision = revision + 1
 WHERE key = 'agent.default_provider_id' AND jsonb_typeof(value) = 'string';
