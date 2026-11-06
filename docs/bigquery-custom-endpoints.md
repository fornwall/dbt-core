# BigQuery custom endpoints

Set `api_endpoint` in a BigQuery profile to route REST requests through a proxy
or emulator. With this setting alone, dbt also fetches query results over REST,
matching dbt Core 1.x. The endpoint must be an HTTP or HTTPS base URL, with no
path other than an optional trailing slash.

For example, a local smallquery emulator accepts this profile:

```yaml
smallquery:
  target: local
  outputs:
    local:
      type: bigquery
      method: oauth-secrets
      token: fake-token
      project: test
      dataset: dbt_probe
      location: US
      threads: 1
      api_endpoint: http://localhost:9050/
```

To fetch results through a separate Storage Read endpoint, set
`storage_read_api_endpoint` as well. This opts out of the REST default. The
existing endpoint override requires the Foundry driver
(`DBT_ADBC_BIGQUERY_USE_FOUNDRY=1`); the default driver does not honor it. The
current Foundry driver uses plaintext gRPC for a custom Storage Read endpoint,
so this override is intended for emulators, for example
`storage_read_api_endpoint: http://localhost:9060/`.

Profiles without `api_endpoint` retain the usual Storage Read behavior. Explicit
statement options take precedence over the REST default, including the option
used by `insert_overwrite` to read partition pseudo-columns.
