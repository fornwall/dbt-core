{# Spanner GoogleSQL has no DATETIME type (only TIMESTAMP and DATE), so we use
   TIMESTAMP_DIFF instead of BigQuery's DATETIME_DIFF.
   NOTE: Spanner's TIMESTAMP_DIFF only supports dateparts up to DAY. For
   WEEK/MONTH/QUARTER/YEAR, cast to DATE and use DATE_DIFF instead.
   TODO(spanner): branch on datepart to use DATE_DIFF for calendar parts. #}
{% macro spanner__datediff(first_date, second_date, datepart) -%}

    timestamp_diff(
        cast({{ second_date }} as timestamp),
        cast({{ first_date }} as timestamp),
        {{ datepart }}
    )

{%- endmacro %}
