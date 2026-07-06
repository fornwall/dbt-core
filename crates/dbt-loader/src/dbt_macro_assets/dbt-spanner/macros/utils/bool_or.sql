{% macro spanner__bool_or(expression) -%}

    logical_or({{ expression }})

{%- endmacro %}
