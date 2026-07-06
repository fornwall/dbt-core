{# TODO: verify on Spanner. dbt.default__hash typically wraps md5(); confirm Spanner exposes an md5()-compatible hashing function returning bytes. #}
{% macro spanner__hash(field) -%}
    to_hex({{dbt.default__hash(field)}})
{%- endmacro %}
