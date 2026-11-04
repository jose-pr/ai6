//! Moneypenny's conversation log and each Session's context in SQL: one row per log entry, and
//! one row per provider Session's context with its content as text.

use std::io;

use super::db::Row;
use super::{SqlRegistry, from_json, row_limit, to_json};
use crate::bounds::Limit;
use crate::directorates::registry::assistant_log::{
    AssistantConversation, AssistantLogEntry, AssistantLogStore, entry_text,
};
use crate::directorates::registry::contexts::{SessionContext, SessionContextStore};
use crate::directorates::registry::store::invalid_data;

fn micros_to_seconds(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

fn conversation_of(row: &Row) -> io::Result<AssistantConversation> {
    let count = |index| {
        row.int(index)
            .and_then(|value| u64::try_from(value).map_err(|error| invalid_data(error.to_string())))
    };
    Ok(AssistantConversation {
        conversation: row.text(0)?,
        person: row.text(1)?,
        first_at: micros_to_seconds(row.int(2)?),
        last_at: micros_to_seconds(row.int(3)?),
        entries: count(4)?,
        bytes: count(5)?,
    })
}

impl AssistantLogStore for SqlRegistry {
    fn log_assistant_entry(&self, entry: &AssistantLogEntry) -> io::Result<()> {
        let text = entry_text(entry, &self.limits.assistant_log)?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_assistant_log \
                 (id, conversation, person, at_micros, bytes, entry, revision) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    entry.id.as_str().into(),
                    entry.conversation.as_str().into(),
                    entry.person.as_str().into(),
                    i64::try_from(entry.micros()).unwrap_or(i64::MAX).into(),
                    text.len().into(),
                    text.as_str().into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn assistant_log(&self, conversation: &str) -> io::Result<Vec<AssistantLogEntry>> {
        let rows = self.read(|tx| {
            tx.query(
                "SELECT entry FROM ai6_assistant_log WHERE conversation = $1 \
                 ORDER BY at_micros, id",
                &[conversation.into()],
            )
        })?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(entry) =
                AssistantLogEntry::from_record(row.text(0)?.as_bytes(), &self.limits.assistant_log)
                && entry.conversation == conversation
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn assistant_conversations(
        &self,
        person: Option<&str>,
    ) -> io::Result<Vec<AssistantConversation>> {
        const SUMMARY: &str = "SELECT conversation, min(person), min(at_micros), max(at_micros), \
                               count(*), CAST(sum(bytes) AS BIGINT) FROM ai6_assistant_log";
        let rows = self.read(|tx| match person {
            Some(person) => tx.query(
                &format!(
                    "{SUMMARY} WHERE person = $1 GROUP BY conversation \
                     ORDER BY max(at_micros) DESC, conversation"
                ),
                &[person.into()],
            ),
            None => tx.query(
                &format!(
                    "{SUMMARY} GROUP BY conversation ORDER BY max(at_micros) DESC, conversation"
                ),
                &[],
            ),
        })?;
        rows.iter().map(conversation_of).collect()
    }

    fn prune_assistant_log(&self, before: Option<f64>, bytes: Limit) -> io::Result<u64> {
        self.write(|tx, _| {
            let mut removed = 0;
            if let Some(before) = before {
                let cutoff = (before.max(0.0) * 1_000_000.0) as i64;
                removed += tx.execute(
                    "DELETE FROM ai6_assistant_log WHERE at_micros < $1",
                    &[cutoff.into()],
                )?;
            }
            if bytes.get().is_some() {
                removed += tx.execute(
                    "DELETE FROM ai6_assistant_log WHERE id IN (SELECT id FROM (SELECT id, \
                     CAST(sum(bytes) OVER (ORDER BY at_micros DESC, id DESC) AS BIGINT) AS kept \
                     FROM ai6_assistant_log) AS ranked WHERE kept > $1)",
                    &[row_limit(bytes).into()],
                )?;
            }
            Ok(removed)
        })
    }
}

fn context_of(row: &Row) -> io::Result<SessionContext> {
    let mut context: SessionContext = from_json(&row.text(0)?, "Session context")?;
    context.content = row.text(1)?;
    Ok(context)
}

impl SessionContextStore for SqlRegistry {
    fn file_session_context(&self, context: &SessionContext) -> io::Result<()> {
        context.validate()?;
        let limits = &self.limits.contexts;
        let context = context.clone().cut_to(limits.context_bytes);
        let record = to_json(&context)?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_session_contexts \
                 (session, conversation, operation, filed_micros, record, content, revision) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (session) DO UPDATE SET conversation = excluded.conversation, \
                 operation = excluded.operation, filed_micros = excluded.filed_micros, \
                 record = excluded.record, content = excluded.content, \
                 revision = excluded.revision",
                &[
                    context.session.as_str().into(),
                    context.conversation().unwrap_or_default().into(),
                    context.operation().unwrap_or_default().into(),
                    context.micros().into(),
                    record.as_str().into(),
                    context.content.as_str().into(),
                    revision.into(),
                ],
            )?;
            let count = tx
                .query_opt("SELECT count(*) FROM ai6_session_contexts", &[])?
                .map_or(Ok(0), |row| row.int(0))?;
            let excess = count.saturating_sub(row_limit(limits.records));
            if excess > 0 {
                tx.execute(
                    "DELETE FROM ai6_session_contexts WHERE session IN (SELECT session FROM \
                     ai6_session_contexts ORDER BY filed_micros, session LIMIT $1)",
                    &[excess.into()],
                )?;
            }
            Ok(())
        })
    }

    fn session_context(&self, session: &str) -> io::Result<Option<SessionContext>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT record, content FROM ai6_session_contexts WHERE session = $1",
                &[session.into()],
            )
        })?
        .map(|row| context_of(&row))
        .transpose()
    }

    fn conversation_contexts(&self, conversation: &str) -> io::Result<Vec<SessionContext>> {
        self.read(|tx| {
            tx.query(
                "SELECT record, content FROM ai6_session_contexts WHERE conversation = $1 \
                 ORDER BY filed_micros, session",
                &[conversation.into()],
            )
        })?
        .iter()
        .map(context_of)
        .collect()
    }
}
