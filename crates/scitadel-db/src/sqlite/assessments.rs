use rusqlite::params;
use scitadel_core::error::CoreError;
use scitadel_core::models::{Assessment, AssessmentId, PaperId, QuestionId};
use scitadel_core::ports::AssessmentRepository;

use super::Database;
use crate::error::DbError;

pub struct SqliteAssessmentRepository {
    db: Database,
}

impl SqliteAssessmentRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn row_to_assessment(row: &rusqlite::Row) -> rusqlite::Result<Assessment> {
    let id: String = row.get("id")?;
    let paper_id: String = row.get("paper_id")?;
    let question_id: String = row.get("question_id")?;
    let created_at: String = row.get("created_at")?;

    Ok(Assessment {
        id: AssessmentId::from(id),
        paper_id: PaperId::from(paper_id),
        question_id: QuestionId::from(question_id),
        score: row.get("score")?,
        reasoning: row.get("reasoning")?,
        model: row.get("model")?,
        prompt: row.get("prompt")?,
        temperature: row.get("temperature")?,
        assessor: row.get("assessor")?,
        created_at: super::parse_rfc3339_or_now(&created_at),
    })
}

impl AssessmentRepository for SqliteAssessmentRepository {
    fn save(&self, assessment: &Assessment) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO assessments
                (id, paper_id, question_id, score, reasoning, model,
                 prompt, temperature, assessor, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                assessment.id.as_str(),
                assessment.paper_id.as_str(),
                assessment.question_id.as_str(),
                assessment.score,
                assessment.reasoning,
                assessment.model,
                assessment.prompt,
                assessment.temperature,
                assessment.assessor,
                assessment.created_at.to_rfc3339(),
            ],
        )
        .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get_for_paper(
        &self,
        paper_id: &str,
        question_id: Option<&str>,
    ) -> Result<Vec<Assessment>, CoreError> {
        let conn = self.db.conn()?;
        let (sql, query_params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(qid) = question_id {
                (
                    "SELECT * FROM assessments WHERE paper_id = ?1 AND question_id = ?2",
                    vec![Box::new(paper_id.to_string()), Box::new(qid.to_string())],
                )
            } else {
                (
                    "SELECT * FROM assessments WHERE paper_id = ?1",
                    vec![Box::new(paper_id.to_string())],
                )
            };
        let mut stmt = conn.prepare(sql).map_err(DbError::Sqlite)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            query_params.iter().map(|b| b.as_ref()).collect();
        let assessments = stmt
            .query_map(params.as_slice(), row_to_assessment)
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(assessments)
    }

    fn get_for_question(&self, question_id: &str) -> Result<Vec<Assessment>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM assessments WHERE question_id = ?1")
            .map_err(DbError::Sqlite)?;
        let assessments = stmt
            .query_map(params![question_id], row_to_assessment)
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(assessments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::{Database, SqlitePaperRepository, SqliteQuestionRepository};
    use scitadel_core::models::{Paper, ResearchQuestion};
    use scitadel_core::ports::{PaperRepository, QuestionRepository};

    #[test]
    fn test_assessment_crud() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let paper_repo = SqlitePaperRepository::new(db.clone());
        let q_repo = SqliteQuestionRepository::new(db.clone());
        let a_repo = SqliteAssessmentRepository::new(db);

        let paper = Paper::new("Test Paper");
        paper_repo.save(&paper).unwrap();

        let q = ResearchQuestion::new("Test question");
        q_repo.save_question(&q).unwrap();

        let assessment = Assessment::new(paper.id.clone(), q.id.clone(), 0.85);
        a_repo.save(&assessment).unwrap();

        let loaded = a_repo
            .get_for_paper(paper.id.as_str(), Some(q.id.as_str()))
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert!((loaded[0].score - 0.85).abs() < f64::EPSILON);
    }
}
