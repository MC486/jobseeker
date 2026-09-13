-- Clearance-to-start vs obtain, citizenship on the profile, and the
-- columns matching needs so preference (comp/location) can stay out of
-- the qualification score.

ALTER TABLE job ADD COLUMN clearance_required_to_start INTEGER
    CHECK (clearance_required_to_start IS NULL OR clearance_required_to_start IN (0, 1));

ALTER TABLE profile ADD COLUMN citizenship TEXT
    CHECK (citizenship IS NULL OR citizenship IN ('us', 'other'));

ALTER TABLE profile ADD COLUMN clearance_held TEXT;

ALTER TABLE profile ADD COLUMN can_obtain_clearance INTEGER
    CHECK (can_obtain_clearance IS NULL OR can_obtain_clearance IN (0, 1));
