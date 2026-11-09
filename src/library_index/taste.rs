use super::{Index, Kind, Stamp};
use crate::{
    background,
    files::Result,
    model::{Record, Source},
    recommendations::{Item, Vectors},
    search_worker::{DIMENSIONS, MODEL},
};
use rusqlite::params;
use std::{collections::HashMap, path::PathBuf};

impl Index {
    pub fn taste_vectors(&self, items: &[Item]) -> Result<Vectors> {
        let wanted: HashMap<_, _> = items
            .iter()
            .map(|item| {
                let identity = match &item.record.source {
                    Source::File(path) => std::fs::canonicalize(path)
                        .ok()
                        .map(Source::File)
                        .unwrap_or_else(|| item.record.source.clone()),
                    source => source.clone(),
                };
                (identity, item)
            })
            .collect();
        let mut query = self.db.prepare(
            "SELECT l.path,l.origin,l.timeline,l.bytes,l.modified,l.content,r.kind
             FROM locations l JOIN roots r ON r.key=l.root
             JOIN builds b ON b.content=l.content AND b.model=?1
             WHERE l.error IS NULL AND b.next>0 ORDER BY r.kind,l.id",
        )?;
        let mut lookup = self.db.prepare_cached(
            "SELECT start,end,ordinal FROM passages WHERE model=?1 AND content=?2 ORDER BY ordinal",
        )?;
        let mut vectors = self.db.prepare_cached(
            "SELECT vector FROM passages WHERE model=?1 AND content=?2 AND ordinal=?3",
        )?;
        let mut found = HashMap::new();
        let mut rows = query.query([MODEL])?;
        while let Some(row) = rows.next()? {
            background::check()?;
            let path = PathBuf::from(row.get::<_, String>(0)?);
            let kind: i64 = row.get(6)?;
            let identity = row
                .get::<_, Option<String>>(1)?
                .and_then(|json| serde_json::from_str::<Record>(&json).ok())
                .map(|record| record.source)
                .or_else(|| {
                    if kind == Kind::Folder as i64 {
                        Some(Source::File(path.clone()))
                    } else if kind == Kind::Download as i64 {
                        let id = path.file_stem()?.to_str()?;
                        crate::youtube::watch_url(id)?;
                        Some(Source::YouTube { id: id.to_owned() })
                    } else {
                        None
                    }
                });
            let identity = identity.map(|source| {
                if kind == Kind::Cut as i64
                    && !wanted.contains_key(&source)
                    && let Source::File(path) = &source
                    && let Ok(path) = std::fs::canonicalize(path)
                {
                    return Source::File(path);
                }
                source
            });
            let Some(item) = identity.as_ref().and_then(|source| wanted.get(source)) else {
                continue;
            };
            let stamp = Stamp {
                bytes: row.get(3)?,
                modified: row.get(4)?,
            };
            if Stamp::read(&path).ok().as_ref() != Some(&stamp) {
                continue;
            }
            let content: String = row.get(5)?;
            let timeline: f64 = row.get(2)?;
            let passages = lookup
                .query_map(params![MODEL, content], |row| {
                    Ok((
                        row.get::<_, f64>(0)? + timeline,
                        row.get::<_, f64>(1)? + timeline,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if passages.is_empty() {
                continue;
            }
            let mut ordinals = Vec::new();
            let quality = if let Some((start, end)) = item.range.filter(|(start, end)| start < end)
            {
                let mut matches: Vec<_> = passages
                    .iter()
                    .filter_map(|&(left, right, ordinal)| {
                        let overlap = right.min(end) - left.max(start);
                        (overlap > 0.0)
                            .then_some((overlap / (right.max(end) - left.min(start)), ordinal))
                    })
                    .collect();
                matches.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                if let Some(best) = matches.first() {
                    ordinals.extend(
                        matches
                            .iter()
                            .take(3)
                            .map(|&(score, ordinal)| (ordinal, score as f32)),
                    );
                    best.0 + 1.0
                } else if kind == Kind::Cut as i64 {
                    continue;
                } else {
                    0.0
                }
            } else if kind == Kind::Cut as i64 {
                0.0
            } else {
                1.0
            };
            if found
                .get(&item.record.source)
                .is_some_and(|(previous, _)| *previous >= quality)
            {
                continue;
            }
            if ordinals.is_empty() {
                let count = passages.len().min(8);
                for i in 0..count {
                    ordinals.push((
                        passages[i * (passages.len() - 1) / count.saturating_sub(1).max(1)].2,
                        1.0,
                    ));
                }
            }
            let mut vector = vec![0.0_f32; DIMENSIONS];
            for (ordinal, weight) in ordinals {
                let bytes: Vec<u8> =
                    vectors.query_row(params![MODEL, content, ordinal], |row| row.get(0))?;
                if bytes.len() != DIMENSIONS * 4 {
                    continue;
                }
                for (value, &bytes) in vector.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *value += f32::from_le_bytes(bytes) * weight;
                }
            }
            let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            if norm.is_finite() && norm > 1e-6 {
                for value in &mut vector {
                    *value /= norm;
                }
                found.insert(item.record.source.clone(), (quality, vector));
            }
        }
        Ok(found
            .into_iter()
            .map(|(source, (_, vector))| (source, vector))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_matching_passage_and_rejects_stale_vectors() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("record.wav");
        std::fs::write(&path, [0; 24])?;
        let mut index = Index::open(&folder.path().join("index"))?;
        index.add_folder(folder.path(), "")?;
        index.scan()?;
        index
            .db
            .execute("UPDATE locations SET content='record'", [])?;
        index.db.execute(
            "INSERT INTO builds(content,model,next,done) VALUES('record',?1,2,1)",
            [MODEL],
        )?;
        for ordinal in 0..2 {
            let mut vector = vec![0.0_f32; DIMENSIONS];
            vector[ordinal] = 1.0;
            let blob: Vec<_> = vector.into_iter().flat_map(f32::to_le_bytes).collect();
            index.db.execute(
                "INSERT INTO passages(content,model,ordinal,start,end,vector,digest)
                 VALUES('record',?1,?2,?3,?4,?5,'test')",
                params![MODEL, ordinal, ordinal * 8, (ordinal + 1) * 8, blob],
            )?;
        }
        let record = crate::files::local_record(path.clone());
        let mut item = Item {
            record,
            range: Some((9.0, 11.0)),
        };
        let vector = index.taste_vectors(&[item.clone()])?;
        assert_eq!(&vector[&item.record.source][..2], &[0.0, 1.0]);
        item.range = None;
        let vector = index.taste_vectors(&[item.clone()])?;
        assert!((vector[&item.record.source][0] - 0.5_f32.sqrt()).abs() < 1e-5);
        std::fs::write(path, [0; 25])?;
        assert!(index.taste_vectors(&[item])?.is_empty());
        Ok(())
    }
}
