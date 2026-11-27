-- What a provider's models source last listed is kept in a row of its own, beside the provider's facts.
-- The list can be megabytes: an OpenRouter catalogue is about 9 MB of JSON, 2 MB stored. Only the places that draw it
-- read it (the provider's model list and adding a model to it). Every check, list and view of a provider reads the
-- provider's row, and every update of the row compares the old and the new row whole (its revision trigger); with the
-- list among the facts, each of them read the whole list again, 60-120 ms a row.
CREATE TABLE provider_model_lists (
    provider_id uuid NOT NULL,
    -- The models as the source listed them, in its order.
    models jsonb NOT NULL,
    -- How many it lists: what a provider's view says, read without the list.
    listed integer GENERATED ALWAYS AS (jsonb_array_length(models)) STORED,
    CONSTRAINT provider_model_lists_pkey PRIMARY KEY (provider_id),
    CONSTRAINT provider_model_lists_models_check CHECK (jsonb_typeof(models) = 'array'),
    CONSTRAINT provider_model_lists_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
);

COMMENT ON TABLE provider_model_lists IS 'The model list a provider''s models source last reported; kept apart from providers.facts, which holds every other member of what its sources report.';

-- The lists already stored move out of the facts; a member that is no list (null) says no more than its absence.
INSERT INTO provider_model_lists (provider_id, models)
SELECT id, facts -> 'models' FROM providers WHERE jsonb_typeof(facts -> 'models') = 'array';
UPDATE providers SET facts = facts - 'models' WHERE facts ? 'models';
