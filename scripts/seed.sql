-- ide99-bench seed schema.
-- Two tables drive the read-path benchmarks:
--   public.events_10m  — 10,000,000 rows, mixed types incl. JSONB
--   public.lookup_1k   — 1,000 rows, used for joins / EXPLAIN tests
--
-- Generation uses generate_series + md5/random so the data is reproducible
-- enough for ranking but not memorisable across runs (timing immune to
-- planner short-circuits like trivial constant folding).

DROP TABLE IF EXISTS public.events_10m;
DROP TABLE IF EXISTS public.lookup_1k;

CREATE UNLOGGED TABLE public.lookup_1k (
    id          int  PRIMARY KEY,
    name        text NOT NULL,
    region      text NOT NULL
);

INSERT INTO public.lookup_1k (id, name, region)
SELECT  g,
        'lookup_' || g,
        (ARRAY['eu-west','us-east','ap-south','sa-east'])[1 + (g % 4)]
FROM generate_series(1, 1000) AS g;

CREATE UNLOGGED TABLE public.events_10m (
    id          bigint PRIMARY KEY,
    created_at  timestamptz NOT NULL,
    user_id     int NOT NULL,
    lookup_id   int NOT NULL,
    amount      numeric(12,2) NOT NULL,
    status      text NOT NULL,
    payload     jsonb NOT NULL,
    note        text
);

-- 10M rows. ~1.7 GiB on disk on PG17. UNLOGGED skips WAL — we don't need
-- crash safety in a throwaway bench DB and it cuts seed time roughly 2x.
INSERT INTO public.events_10m
SELECT  g                                            AS id,
        now() - (g % 365 || ' days')::interval       AS created_at,
        1 + (g % 100000)                             AS user_id,
        1 + (g % 1000)                               AS lookup_id,
        ((g % 100000)::numeric / 100)                AS amount,
        (ARRAY['ok','pending','failed','retried'])[1 + (g % 4)] AS status,
        jsonb_build_object(
            'tag',     md5(g::text),
            'flags',   ARRAY[g % 7, g % 11, g % 13],
            'meta',    jsonb_build_object('src', 'seed', 'batch', g / 1000000)
        )                                            AS payload,
        CASE WHEN g % 50 = 0 THEN repeat('x', 1 + g % 200) END AS note
FROM generate_series(1, 10000000) AS g;

-- Useful indexes for the EXPLAIN / read-path bench.
CREATE INDEX events_10m_created_at_idx ON public.events_10m (created_at);
CREATE INDEX events_10m_user_id_idx    ON public.events_10m (user_id);
CREATE INDEX events_10m_status_idx     ON public.events_10m (status);

ANALYZE public.events_10m;
ANALYZE public.lookup_1k;

SELECT 'seed done', count(*) FROM public.events_10m;
