{{ config(materialized='incremental', unique_key='id', primary_key=['id']) }}

{% if is_incremental() %}
-- Incremental run: an overlapping unique key plus a new one, to exercise the
-- delete+insert upsert. id=1 already exists and its value changes (so it must be
-- deleted then re-inserted, not duplicated); id=3 is new; id=2 is left untouched.
select 1 as id, 'HELLO_UPDATED' as val
union all
select 3 as id, 'three' as val
{% else %}
-- First run: seed from the upstream table (ids 1 and 2).
select id, greeting as val
from {{ ref('spanner_table') }}
{% endif %}
