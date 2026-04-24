use rusqlite::{OptionalExtension, params};
use scitadel_core::error::CoreError;
use scitadel_core::models::{QuestionId, ResearchQuestion, SearchTerm, SearchTermId};
use scitadel_core::ports::QuestionRepository;

use super::Database;
use crate::error::DbError;

pub struct SqliteQuestionRepository {
    db: Database,
}

impl SqliteQuestionRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

impl QuestionRepository for SqliteQuestionRepository {
    fn save_question(&self, question: &ResearchQuestion) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO research_questions
                (id, text, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                question.id.as_str(),
                question.text,
                question.description,
                question.created_at.to_rfc3339(),
                question.updated_at.to_rfc3339(),
            ],
        )
        .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get_question(&self, question_id: &str) -> Result<Option<ResearchQuestion>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM research_questions WHERE id = ?1")
            .map_err(DbError::Sqlite)?;
        let result = stmt
            .query_row(params![question_id], |row| {
                let id: String = row.get("id")?;
                let created_at: String = row.get("created_at")?;
                let updated_at: String = row.get("updated_at")?;
                Ok(ResearchQuestion {
                    id: QuestionId::from(id),
                    text: row.get("text")?,
                    description: row.get("description")?,
                    created_at: super::parse_rfc3339_or_now(&created_at),
                    updated_at: super::parse_rfc3339_or_now(&updated_at),
                })
            })
            .optional()
            .map_err(DbError::Sqlite)?;
        Ok(result)
    }

    fn list_questions(&self) -> Result<Vec<ResearchQuestion>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM research_questions ORDER BY created_at DESC")
            .map_err(DbError::Sqlite)?;
        let questions = stmt
            .query_map([], |row| {
                let id: String = row.get("id")?;
                let created_at: String = row.get("created_at")?;
                let updated_at: String = row.get("updated_at")?;
                Ok(ResearchQuestion {
                    id: QuestionId::from(id),
                    text: row.get("text")?,
                    description: row.get("description")?,
                    created_at: super::parse_rfc3339_or_now(&created_at),
                    updated_at: super::parse_rfc3339_or_now(&updated_at),
                })
            })
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(questions)
    }

    fn save_term(&self, term: &SearchTerm) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO search_terms
                (id, question_id, terms, query_string, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                term.id.as_str(),
                term.question_id.as_str(),
                serde_json::to_string(&term.terms).unwrap_or_default(),
                term.query_string,
                term.created_at.to_rfc3339(),
            ],
        )
        .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get_terms(&self, question_id: &str) -> Result<Vec<SearchTerm>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM search_terms WHERE question_id = ?1")
            .map_err(DbError::Sqlite)?;
        let terms = stmt
            .query_map(params![question_id], |row| {
                let id: String = row.get("id")?;
                let question_id: String = row.get("question_id")?;
                let terms_json: String = row.get("terms")?;
                let created_at: String = row.get("created_at")?;
                Ok(SearchTerm {
                    id: SearchTermId::from(id),
                    question_id: QuestionId::from(question_id),
                    terms: serde_json::from_str(&terms_json).unwrap_or_default(),
                    query_string: row.get("query_string")?,
                    created_at: super::parse_rfc3339_or_now(&created_at),
                })
            })
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(terms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::Database;

    #[test]
    fn test_question_crud() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let repo = SqliteQuestionRepository::new(db);

        let q = ResearchQuestion::new("What is dark matter?");
        repo.save_question(&q).unwrap();

        let loaded = repo.get_question(q.id.as_str()).unwrap().unwrap();
        assert_eq!(loaded.text, "What is dark matter?");

        let all = repo.list_questions().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_search_terms() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let repo = SqliteQuestionRepository::new(db);

        let q = ResearchQuestion::new("Test question");
        repo.save_question(&q).unwrap();

        let mut term = SearchTerm::new(q.id.clone());
        term.terms = vec!["dark".into(), "matter".into()];
        term.query_string = "dark matter".into();
        repo.save_term(&term).unwrap();

        let terms = repo.get_terms(q.id.as_str()).unwrap();
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].query_string, "dark matter");
    }
}
