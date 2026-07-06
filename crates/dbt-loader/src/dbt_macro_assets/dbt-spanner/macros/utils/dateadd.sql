{# Spanner GoogleSQL has no DATETIME type (only TIMESTAMP and DATE), so we use
   TIMESTAMP_ADD instead of BigQuery's DATETIME_ADD.
   NOTE: Spanner's TIMESTAMP_ADD only supports dateparts up to DAY
   (MICROSECOND, MILLISECOND, SECOND, MINUTE, HOUR, DAY). WEEK/MONTH/QUARTER/YEAR
   are not supported by TIMESTAMP_ADD; those require DATE_ADD on a DATE value.
   TODO(spanner): branch on datepart to use DATE_ADD for calendar parts. #}
{% macro spanner__dateadd(datepart, interval, from_date_or_timestamp) %}

    timestamp_add(
        cast( {{ from_date_or_timestamp }} as timestamp),
        interval {{ interval }} {{ datepart }}
    )

{% endmacro %}
