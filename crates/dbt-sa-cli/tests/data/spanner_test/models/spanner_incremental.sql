{{ config(materialized='incremental', unique_key='id', primary_key=['id']) }}
select id, greeting as val
from {{ ref('spanner_table') }}
{% if is_incremental() %}
where id > (select coalesce(max(id), 0) from {{ this }})
{% endif %}
