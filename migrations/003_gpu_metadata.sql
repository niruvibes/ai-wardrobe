BEGIN IMMEDIATE;
CREATE TRIGGER protect_gpu_metadata
BEFORE UPDATE OF data ON garments
WHEN json_type(OLD.data, '$.photo_analysis.recognition_backend') IS NOT NULL
 AND json_type(NEW.data, '$.photo_analysis') = 'object'
 AND json_type(NEW.data, '$.photo_analysis.recognition_backend') IS NULL
BEGIN
    SELECT RAISE(ABORT, 'This item uses GPU analysis metadata. Reopen it in Wardrobe 0.3 or later before editing.');
END;
PRAGMA user_version=3;
COMMIT;
