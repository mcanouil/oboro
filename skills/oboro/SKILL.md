---
name: oboro
description: Read this when text you have been shown holds double-bracket placeholders such as `[[EMAIL_1]]`, `[[PERSON_2]]` or `[[PHONE_1]]`, when a tool result says oboro withheld it, when a tool call is refused by oboro, or when you are asked about anonymising a document before it reaches a model. Oboro is a local anonymisation layer, and its hooks change what you read and what you write.
---

# Oboro

Oboro replaces personal data with placeholders before a model sees it, and puts the real values back afterwards.
The mapping lives in an encrypted vault on the user's machine.

When its hooks are installed, they sit on both sides of your tools.
A `PostToolUse` hook cleans what `Read`, `Grep`, `Bash` and `WebFetch` hand you.
A `PreToolUse` hook restores what `Write` and `Edit` are about to put on disk.

Run `oboro doctor` to see which halves are installed.

## A placeholder stands for a value you cannot see

`[[EMAIL_1]]` is a real email address.
Oboro replaced it on the way to you, and the real value is in the vault.

It is not a bug in the file.
It is not a template to fill in.
It is not a redaction to work around by reading the file another way.

The shape is `[[TAG_n]]`: an uppercase tag, an underscore, a number.
Common tags are `PERSON`, `ORG`, `ADDRESS`, `PHONE`, `EMAIL`, `IBAN`, `CARD`, `SIREN`, `SIRET` and `IP`, plus any custom tag the user's `oboro.toml` defines.
The number is stable, so `[[PERSON_1]]` is the same person everywhere in a session.

Treat the tag as the type and the number as the identity.
You can reason about a file, refactor around it, and answer questions about it without ever knowing what a placeholder stands for.

## Write placeholders back verbatim

If you are writing text that came from a cleaned source, copy the placeholder exactly as you received it.
The `PreToolUse` hook turns `[[EMAIL_1]]` back into the real address before `Write` or `Edit` touches the file, so the user's file ends up correct.

Do not paraphrase a placeholder, guess at the value behind it, or substitute something plausible.
An invented address written into a file is a real error; a placeholder written into a file is restored.

Never invent a placeholder that was not in the text you were given.
A placeholder the vault never issued is left in place and reported to the user, so it lands in the file as literal `[[TAG_n]]` text.

## When Oboro fails, it fails closed

You may see either of these.
Both mean Oboro could not do its job, not that the tool misbehaved.

- `[oboro withheld this tool result: it could not be anonymised]`, with the call blocked. The tool ran and its output was thrown away rather than shown to you uncleaned.
- A `PreToolUse` denial saying the call was refused rather than write placeholders into a file. The tool did not run.

Do not retry, and do not route around it with a different tool.
The reason went to the user, not to you, because it usually names a path and a path is one of the things Oboro redacts.
Tell the user what you were trying to do and stop.

## What the hooks do not cover

What the user types into the chat is never cleaned.
The event that fires on a prompt can add context but cannot rewrite it, so a value typed by hand reaches the model as typed.
If a user is about to paste sensitive text, suggest they let a tool read the file instead, so the hook covers it.

Object keys are not cleaned either.
A tool that keys its result by file path still shows that path, even when the strings inside are clean.

## Using Oboro by hand

In a tool with no hooks, these are the primary path rather than a fallback.

```bash
oboro clean contract.docx      # write a sanitised copy
oboro restore answer.md        # put the real values back
oboro map list                 # see the placeholders issued so far
oboro doctor                   # what this build does, and which hooks are on
```

`clean` and `restore` both read standard input when given `-`, so text held in memory can go through them without a file.

Never put a real value into a bug report, a commit message or an issue.
Oboro exists to keep exactly those values off other people's machines.
