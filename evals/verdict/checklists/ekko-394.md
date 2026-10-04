1. When the text of a create or an edit in a batch holds $N for an N the batch defines, the batch's reply carries a notice. The notice gives the operation, the $N, the fact that batch does not replace it, and the item $N stands for, e.g. 'operation 3's text holds $2, which batch does not replace: that item is 394'. The notice appears in the reply the caller receives, not only in a log or on stderr. The SEEN case gets one: an edit appends 'task $2' when operation 2 created a task.
2. The text is stored exactly as written: $N is not substituted, escaped or annotated. The operation and the batch succeed as before. The notice is information only, never an error, a rejection or a rollback, so note 335's rule stands.
3. The id in the notice is the item operation N created, found the way batch itself resolves $N, not the Nth item created (note 392 (2)).
   - Example: operation 1 is an edit, operation 2 creates 394, operation 3 creates 395. A '$2' in operation 4's text is reported as 394.
   - The operation number in the notice counts operations the same way $N does.
4. Both creates and edits are checked, across the text they write (e.g. a create's title or body, an edit's replaced or appended note). Fields where batch does resolve $N (e.g. the item an edit targets, a parent) are not checked, since 'does not replace' would be false there.
5. There is no notice when the batch defines no such N: N past the last operation, or an operation that created no item.
   - Digits are read whole: '$12' is N=12, not $1.
   - A batch with no $N in its text gets the same reply as before.
6. Every case is reported. Each operation whose text holds a defined $N gets a notice. A text holding several (say $2 and $3) gets one notice per N, each naming its own item.
7. Edge readings of 'holds $N' are free, as long as the reading is consistent and never names the wrong item. This covers whether a $N naming a later operation, or the operation's own new item, is reported, and whether '$2.50' or '$2nd' count (open).
8. Presentation is free: where the notices sit (after the operation results, as a field of structured output, or beside the operation's own result), any wording beyond item 1's parts, and whether a $N repeated in one text is reported once (open).
