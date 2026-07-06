{# NOTE: spanner__current_timestamp lives in macros/adapters.sql to keep it alongside
   the other dispatch adapter macros (and to avoid a duplicate macro definition). #}

{% macro spanner__snapshot_string_as_time(timestamp) -%}
    {%- set result = 'TIMESTAMP("' ~ timestamp ~ '")' -%}
    {{ return(result) }}
{%- endmacro %}

{% macro spanner__current_timestamp_backcompat() -%}
  current_timestamp
{%- endmacro %}
