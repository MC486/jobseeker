-- Seed data: the source registry.
--
-- `fidelity` is the tie-breaker when two listings describe the same posting. An employer's
-- own applicant tracking system is more trustworthy than an aggregator's reconstruction of
-- it, and aggregators frequently publish their own salary *estimates* as if they were the
-- employer's posted band.

INSERT INTO source (id, kind, name, base_url, fidelity, created_at) VALUES
  ('01900000-0000-7000-8000-000000000001', 'manual',          'Manual entry',    NULL,                              100, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000002', 'greenhouse',      'Greenhouse',      'https://boards.greenhouse.io',     90, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000003', 'lever',           'Lever',           'https://jobs.lever.co',            90, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000004', 'ashby',           'Ashby',           'https://jobs.ashbyhq.com',         90, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000005', 'smartrecruiters', 'SmartRecruiters', 'https://jobs.smartrecruiters.com', 90, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000006', 'workable',        'Workable',        'https://apply.workable.com',       90, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000007', 'recruitee',       'Recruitee',       NULL,                               90, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000008', 'workday',         'Workday',         NULL,                               75, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000009', 'company_site',    'Company website', NULL,                               70, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-00000000000a', 'linkedin',        'LinkedIn',        'https://www.linkedin.com',         45, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-00000000000b', 'indeed',          'Indeed',          'https://www.indeed.com',           40, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-00000000000c', 'dice',            'Dice',            'https://www.dice.com',             35, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-00000000000d', 'glassdoor',       'Glassdoor',       'https://www.glassdoor.com',        30, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-00000000000e', 'ziprecruiter',    'ZipRecruiter',    'https://www.ziprecruiter.com',     30, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-00000000000f', 'email',           'Email alert',     NULL,                               20, '2026-01-01T00:00:00Z'),
  ('01900000-0000-7000-8000-000000000010', 'other',           'Other',           NULL,                               10, '2026-01-01T00:00:00Z');

INSERT INTO setting (key, value_json, updated_at) VALUES
  ('schema_notes', '"See docs/04-data-model.md for the reasoning behind this schema."', '2026-01-01T00:00:00Z');
