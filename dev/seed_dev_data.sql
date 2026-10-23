-- Dev-only synthetic data for eyeballing the analytics dashboard.
-- Safe to run against an EMPTY auditor.db (schema v5). Idempotent-ish: it
-- clears the analytic tables first, so re-running refreshes the sample set.
-- Usage: sqlite3 ~/.cashu-mint-auditor/auditor.db < scripts/seed_dev_data.sql
--
-- NB: random() is materialized into TEMP tables before use. Referencing a
-- random() expression directly across several columns of one SELECT re-rolls
-- it per reference in SQLite, which would desync (e.g.) a row's state from its
-- stage/category. Locking the rolls into a temp table first avoids that.

PRAGMA foreign_keys = OFF;

DELETE FROM swaps;
DELETE FROM mint_checks;
DELETE FROM mint_snapshots;
DELETE FROM donations;
DELETE FROM mints;
DELETE FROM sqlite_sequence WHERE name IN ('swaps','mint_checks','mints','donations');

DROP TABLE IF EXISTS _swaps;
DROP TABLE IF EXISTS _checks;
DROP TABLE IF EXISTS _dons;

-- ---- Mints ---------------------------------------------------------------
INSERT INTO mints (id, url, name, info, sum_donations, state, n_errors, n_mints, n_melts, updated_at, latitude, longitude) VALUES
 (1,'https://mint.minibits.cash','Minibits','{"version":"Nutshell/0.16.0"}',0,'OK',2,140,138,strftime('%Y-%m-%dT%H:%M:%SZ','now'),52.52,13.40),
 (2,'https://stablenut.umint.cash','Stablenut','{"version":"Nutshell/0.15.1"}',0,'OK',5,120,118,strftime('%Y-%m-%dT%H:%M:%SZ','now'),40.71,-74.01),
 (3,'https://mint.coinos.io','Coinos','{"version":"cdk-mintd/0.11.0"}',0,'WARN',9,90,84,strftime('%Y-%m-%dT%H:%M:%SZ','now'),51.51,-0.13),
 (4,'https://8333.space:3338','8333.space','{"version":"cdk-mintd/0.10.0"}',0,'ERROR',31,60,40,strftime('%Y-%m-%dT%H:%M:%SZ','now'),35.68,139.69),
 (5,'https://mint.lnvoltz.com','Voltz','{"version":"Nutshell/0.16.0"}',0,'OK',1,160,159,strftime('%Y-%m-%dT%H:%M:%SZ','now'),-23.55,-46.63);

-- ---- Swaps (≈700 over 30 days) ------------------------------------------
CREATE TEMP TABLE _swaps AS
WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM seq WHERE i < 700)
SELECT
  abs(random()) % (30*24*60)         AS mins_ago,
  (abs(random()) % 5) + 1            AS from_id,
  abs(random()) % 4                  AS tojump,
  abs(random()) % 100               AS roll,
  abs(random()) % 4                  AS etype,
  (abs(random()) % 20 + 1) * 10      AS amt,
  500 + abs(random()) % 4500         AS lat
FROM seq;

-- is_err predicate (mint 4 flaky): `roll` is stored, so repeating it is stable.
INSERT INTO swaps (from_id, to_id, from_url, to_url, amount, fee, state, error, failed_mint_id, stage, category, time_taken, created_at)
SELECT
  s.from_id,
  ((s.from_id + s.tojump) % 5) + 1 AS to_id,
  (SELECT url FROM mints WHERE id = s.from_id),
  (SELECT url FROM mints WHERE id = ((s.from_id + s.tojump) % 5) + 1),
  s.amt,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN 0 ELSE (s.lat % (s.amt/10 + 2)) END,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN 'ERROR' ELSE 'OK' END,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN
    CASE s.etype
      WHEN 0 THEN 'mint timeout: did not respond within 10s'
      WHEN 1 THEN 'connection refused by peer'
      WHEN 2 THEN 'Mint error: insufficient_balance to pay invoice'
      ELSE 'keyset rotation: unknown keyset' END
  END,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN ((s.from_id + s.tojump) % 5) + 1 END,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN
    CASE s.etype WHEN 0 THEN 'paying invoice' WHEN 1 THEN 'requesting quote'
                 WHEN 2 THEN 'melting' ELSE 'minting' END
  END,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN
    CASE s.etype WHEN 0 THEN 'timeout' WHEN 1 THEN 'network' WHEN 2 THEN 'liquidity' ELSE 'mint' END
  END,
  CASE WHEN (s.from_id = 4 AND s.roll < 38) OR (s.from_id <> 4 AND s.roll < 11) THEN 0.0 ELSE s.lat * 1.0 END,
  strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now', '-' || s.mins_ago || ' minutes'))
FROM _swaps s;

-- ---- Health probes (hourly per mint, 30 days) ---------------------------
CREATE TEMP TABLE _checks AS
WITH RECURSIVE hrs(h) AS (SELECT 0 UNION ALL SELECT h+1 FROM hrs WHERE h < 24*30)
SELECT
  m.id AS mid, h,
  abs(random()) % 100 AS roll,
  50 + abs(random()) % 350 AS lat
FROM mints m CROSS JOIN hrs;

-- bad predicate: mint 4 very flaky, mint 3 occasionally warns, others rarely fail.
INSERT INTO mint_checks (mint_id, mint_url, kind, state, error, category, latency_ms, created_at)
SELECT
  c.mid,
  (SELECT url FROM mints WHERE id = c.mid),
  'HEALTH',
  CASE
    WHEN c.mid = 3 AND c.roll < 10 THEN 'WARN'
    WHEN c.mid = 4 AND c.roll < 30 THEN 'ERROR'
    WHEN c.mid <> 3 AND c.roll < 3 THEN 'ERROR'
    ELSE 'OK' END,
  CASE WHEN (c.mid = 4 AND c.roll < 30) OR (c.mid <> 3 AND c.roll < 3) THEN 'mint unreachable: connection refused' END,
  CASE WHEN (c.mid = 4 AND c.roll < 30) OR (c.mid <> 3 AND c.roll < 3) THEN 'network' END,
  c.lat * 1.0,
  strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now', '-' || c.h || ' hours'))
FROM _checks c;

-- ---- Balance snapshots (daily per mint, 30 days) ------------------------
WITH RECURSIVE days(d) AS (SELECT 0 UNION ALL SELECT d+1 FROM days WHERE d < 30),
mids(mid, base) AS (
  SELECT 1, 120000 UNION ALL SELECT 2, 80000 UNION ALL SELECT 3, 45000
  UNION ALL SELECT 4, 15000 UNION ALL SELECT 5, 210000
)
INSERT INTO mint_snapshots (ts, mint_id, balance, sum_donations, state, n_errors, n_mints, n_melts, updated_at)
SELECT
  strftime('%Y-%m-%dT12:00:00Z', datetime('now', '-' || d || ' days')),
  mid,
  base + (30 - d) * (base/120) + (abs(random()) % (base/20)) - (base/40),
  0, 'OK', 0, 0, 0,
  strftime('%Y-%m-%dT12:00:00Z', datetime('now', '-' || d || ' days'))
FROM days CROSS JOIN mids;

-- ---- Donations (≈30 over 30 days) ---------------------------------------
CREATE TEMP TABLE _dons AS
WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM seq WHERE i < 30)
SELECT
  (abs(random()) % 5) + 1 AS mid,
  (abs(random()) % 12 + 1) * 100 + 21 AS amt,
  abs(random()) % 2 AS via_roll,
  abs(random()) % (30*24*60) AS mins_ago
FROM seq;

INSERT INTO donations (mint_id, mint_url, amount, via, created_at)
SELECT
  d.mid,
  (SELECT url FROM mints WHERE id = d.mid),
  d.amt,
  CASE WHEN d.via_roll = 0 THEN 'token' ELSE 'nut18' END,
  strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now', '-' || d.mins_ago || ' minutes'))
FROM _dons d;

DROP TABLE _swaps;
DROP TABLE _checks;
DROP TABLE _dons;

SELECT 'mints', count(*) FROM mints
UNION ALL SELECT 'swaps', count(*) FROM swaps
UNION ALL SELECT 'errors', count(*) FROM swaps WHERE state<>'OK'
UNION ALL SELECT 'err w/stage', count(*) FROM swaps WHERE state<>'OK' AND stage IS NOT NULL
UNION ALL SELECT 'checks', count(*) FROM mint_checks
UNION ALL SELECT 'check fails', count(*) FROM mint_checks WHERE state<>'OK'
UNION ALL SELECT 'snapshots', count(*) FROM mint_snapshots
UNION ALL SELECT 'donations', count(*) FROM donations;
