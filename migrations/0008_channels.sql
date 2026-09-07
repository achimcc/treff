-- Zwei Wege aus demselben Postfach.
--
-- Eine Mail geht an JEDEN Abonnenten; ein Webhook geht EINMAL pro Beitrag —
-- ntfy, Gotify und jede Bruecke dahinter verteilen selbst an ihre Abonnenten.
-- Ohne diese Spalte waere ein Webhook pro Person ein Stapel identischer
-- Meldungen auf einem Telefon.
--
-- `subject` bleibt gesetzt und traegt bei einer Webhook-Zeile den Schreiber:
-- Er ist der, der NICHT gemeint ist, und die Zeile braucht trotzdem jemanden,
-- an dem sie haengt.
ALTER TABLE outbox ADD COLUMN kanal TEXT NOT NULL DEFAULT 'mail';

-- Der Sender fragt nach Kanal und Faelligkeit zusammen.
DROP INDEX IF EXISTS outbox_due;
CREATE INDEX outbox_due ON outbox(sent_at, next_try_at, kanal);
