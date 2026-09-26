-- Preserve admitted external-provider operations while tagging their binding.
UPDATE provider_operations
SET binding_json = json_object(
    'kind', 'externalProvider',
    'binding', json(binding_json)
)
WHERE json_extract(binding_json, '$.kind') IS NULL;
