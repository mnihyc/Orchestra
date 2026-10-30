CASE WHEN __SELECTION__->>'file_type'='*/*' THEN true
  WHEN right(__SELECTION__->>'file_type',2)='/*'
    THEN position('/' in __MEDIA_TYPE__)>0 AND split_part(lower(__MEDIA_TYPE__), '/', 1)=left(lower(__SELECTION__->>'file_type'),length(__SELECTION__->>'file_type')-2)
  ELSE lower(__MEDIA_TYPE__)=lower(__SELECTION__->>'file_type')
END
