BEGIN IMMEDIATE;
-- Version 2 marks the visual metadata format. An older app refuses this database
-- on startup. If an older app was already open, prevent a stale save from
-- silently discarding fields it cannot understand on a newly upgraded item.
CREATE TRIGGER protect_visual_metadata
BEFORE UPDATE OF data ON garments
WHEN json_type(OLD.data, '$.description') IS NOT NULL
 AND json_type(NEW.data, '$.description') IS NULL
BEGIN
    SELECT RAISE(ABORT, 'This item uses photo automation. Reopen it in Wardrobe 0.2 or later before editing.');
END;
PRAGMA user_version=2;
COMMIT;
