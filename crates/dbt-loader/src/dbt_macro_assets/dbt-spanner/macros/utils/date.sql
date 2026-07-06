{% macro spanner__date(year, month, day) -%}
    date({{ year }}, {{ month }}, {{ day }})
{%- endmacro %}
