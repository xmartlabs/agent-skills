---
name: homosapiens
description: This skill should be used before publishing any text other people will read (PR titles and bodies, Jira comments, commit messages, docs, Confluence pages) and whenever the user asks to remove AI tells, humanize a text, or make writing sound less AI-generated. It rewrites text to strip AI writing patterns and add human voice without changing what it says. Embedded inside another tool (such as tribunal's PR reviews) it returns only the final text.
---

# Homosapiens

Edit text to remove AI patterns and sound like a person wrote it, without changing what it says.

## Process

1. Scan for the patterns below.
2. Rewrite. Preserve meaning and every claim; match intended tone. Don't add or drop a fact, name, number, date, quote, or citation — if a sentence needs a missing detail, use a simpler sentence instead of inventing one. Fiction is exempt from the no-invention rule.
3. Add soul (see "Adding soul").
4. Self-audit: "What makes this obviously AI generated? Did I lose or invent any claim?" Fix what's left.

## Match an existing voice

If given a writing sample, read it first: sentence length, word choice, paragraph openings, punctuation, repeated phrases. Follow those habits instead of the defaults below — e.g. a sample that uses em dashes overrides the dash rule at its rate.

## Adding soul

Removing patterns is half the job. Sterile, voiceless writing is just as obvious. Use personality where the writer and content call for it (blog posts, essays, opinions); keep reference, technical, and legal text neutral, and work artifacts too (PR bodies, review reports, Jira comments, commit messages): there, soul means rhythm and plain speech, not opinions or first person.

- **Have opinions.** React to facts instead of neutrally listing pros and cons.
- **Vary rhythm.** Short sentences. Then longer ones that take their time. Mix it up.
- **Acknowledge complexity.** "Impressive but also kind of unsettling" beats "impressive."
- **Use "I" when it fits.** First person isn't unprofessional.
- **Let some mess in.** Perfect structure looks machine-made.
- **Be specific.** Not "this is concerning" but "there's something unsettling about agents churning away at 3am."

## Patterns to detect and fix

### Content

1. **Puffery.** "pivotal moment", "testament to", "evolving landscape", "setting the stage for", "indelible mark", "deeply rooted", "focal point". Cut puffery, state what happened.
2. **Name-dropping.** Listing outlets or follower counts without context. Pick one, say what was said.
3. **Superficial -ing phrases.** "highlighting...", "ensuring...", "reflecting...", "showcasing...", "fostering...", "cultivating...". Delete or expand with real sources.
4. **Promotional language.** "nestled", "vibrant", "breathtaking", "groundbreaking", "renowned", "stunning", "must-visit", "in the heart of". Use neutral descriptions.
5. **Vague attributions.** "Experts believe", "Industry reports suggest", "Observers have cited". Name the source or delete.
6. **Formulaic challenges/outlook.** "Despite challenges... continues to thrive." Replace with specific facts or cut.

### Language

7. **AI vocabulary.** Actually, additionally, align with, crucial, delve, emphasizing, enduring, enhance, fostering, garner, gate/gated/gating (figuratively — keep real technical gating), interplay, intricate, key (as filler adjective), landscape (abstract), pivotal, quietly, showcase, tapestry (abstract), testament, underscore, valuable, vibrant. Replace with plain words.
8. **Fancy ways to say "is".** "serves as", "stands as", "boasts", "features", "represents a". Just say "is" or "has".
9. **"Not just X, but Y" and clipped endings.** State the point directly instead. Same for tacked-on negations like "no guessing" — write the full clause.
10. **Rule of three.** Forcing ideas into groups of three. Use the natural number.
11. **Synonym cycling and repeated openings.** Protagonist/main character/hero all in one paragraph — pick one, repeat it. Same fix for several sentences in a row starting with "She"/"He": merge them or vary the opening. Deliberate repetition for rhythm ("She came. She saw. She conquered.") is fine.
12. **False ranges.** "from X to Y" where X and Y aren't on a meaningful scale. List topics directly.
13. **Passive voice, missing subjects.** "No configuration file needed", "queries are validated" hide who acts. Name the actor: "You don't need a config file", "the compiler validates queries". Fine when the actor is unknown or doesn't matter.

### Style

14. **Em/en dash overuse.** Avoid em and en dashes entirely, including spaced ` — ` and double-hyphen ` -- ` substitutes. Use periods or commas. Never introduce a dash in a rewrite. An existing lone dash isn't a tell by itself (many editors use them); it may stay only when no other patterns co-occur.
15. **Colon overuse.** Colons are fine before a list or example, not as mid-sentence connectors. "If you're coming from automation: instead of X, you do Y" adds nothing — let the point stand without comparison framing.
16. **Boldface overuse.** Don't bold every proper noun or acronym.
17. **Inline-header lists.** "**Performance:** Performance improved..." restates the label. Convert to prose. A bold lead-in ending in a period that adds genuinely new detail ("**Schema in TypeScript.** Tables live in one file.") is fine.
18. **Title case headings.** Use sentence case.
19. **Decorative emojis.** Remove from headings and bullets.
20. **Curly quotes.** Replace with straight quotes. One curly quote alone isn't a tell (most editors auto-curl); change it only alongside other patterns.
21. **Hyphenated word pairs.** third-party, cross-functional, data-driven, high-quality everywhere. Keep the hyphen before a noun ("a high-quality report"), drop it after ("the report is high quality").

### Communication artifacts

22. **Chatbot phrases.** "I hope this helps!", "Let me know if...", "Of course!", "Would you like me to...?". Remove.
23. **Cutoff disclaimers and guesses.** "While specific details are limited...", "it is believed that", "likely grew up in...". State what the source shows, or cut the sentence. Never present a guess as fact.
24. **Sycophantic tone.** "Great question! You're absolutely right!" Respond directly.

### Filler

25. **Filler phrases.** "In order to" becomes "To". "Due to the fact that" becomes "Because". "It is important to note that" gets deleted.
26. **Excessive hedging.** "could potentially possibly be argued that it might" becomes "may". Cut caveats that only patch an earlier overstatement.
27. **Generic conclusions.** "The future looks bright." End on the last concrete fact or a sourced plan instead.

### Rhetorical tics

28. **Fake deeper truth.** "At its core, what really matters is..." State the point directly.
29. **Announcing the next point.** "Let's dive in", "here's what you need to know", "one thing that bit me". Start with the content.
30. **Heading repeated below itself.** A heading followed by a line that only restates it. Let the heading do the work.
31. **Writing about the old version.** "This function was added to replace..." belongs in a changelog, not in docs describing current behavior. Describe what it does now.
32. **Forced punchlines.** "It had no preference. No prior. No nostalgia." One short fragment for emphasis is fine; a row of them is forced. Use natural sentence lengths.
33. **Formulaic sayings.** "Symmetry is the language of trust", "X becomes a trap". State the specific claim instead.
34. **Fake-candid openings.** "Honestly? It depends..." State the answer directly. Mid-sentence "honestly" or "look" in casual writing is fine — the tell is the staged standalone opener.
35. **Answering objections no one raised.** "This isn't mainly about X, and I'm not arguing Y..." when X/Y appear nowhere else. Remove the unsupported defense; keep any real claim inside it.
36. **Rejecting fake alternatives.** "A tempting approach would be..., but" introducing an option no reader would consider, dismissed and never mentioned again. Remove it, state the real constraint. A genuinely relevant alternative in a design doc or tutorial is not this.

### Jargon

37. **Abstract metaphor nouns.** Substrate, wedge, vector, locus, vantage, nexus, primitive (as noun), harness (as metaphor), surface (as in "API surface"), bedrock, scaffolding (metaphor), modality, paradigm, gold-plating, ratchet (metaphor), evacuate (for moving code), endgame, north star, flywheel. These read as technical but usually have a plainer concrete word: "substrate" → "base", "vector" → "way"/"method", "gold-plating" → "more than the job needs", "evacuate" → "move out".

### Plain speech

38. **Say what it does, not how it feels.** "the database stays close at hand" names a feeling. Name the mechanism or a number instead: "`.toSQL()` returns the exact string sent to the database". If a sentence could appear unchanged in another project's docs, it says nothing about this one — cut it.
39. **Shorten or split dense sentences.** If the reader has to backtrack to parse it, break it in two or drop clauses. One idea per sentence.
40. **Cut adverbs, or use a stronger verb.** "runs quickly" becomes "is fast" or the number. An adverb propping up a weak verb means the verb is wrong.
41. **Prefer the plain word.** "utilize" → "use", "leverage" → "use", "facilitate" → "help", "numerous" → "many", "in the event that" → "if".

## What to leave alone

A person may use some of these patterns without being AI. Don't rewrite on the strength of any single item by itself:

- Perfect grammar, polish, or formal/academic words (only §7's specific list is the tell, not formality in general).
- One em dash, one curly quote, or one short fragment alone: rewrite only when stacked with other patterns.
- Deliberate repeated openings, mixed casual/formal register, or a letter-style opening/closing (predates AI).
- Real, sourced disclaimers, limits, or alternatives a reader would genuinely consider — cut only the fake or unsupported ones.
- Secondhand text: quotations, titles, proper names, or examples discussing a watched phrase rather than using it.
- Unsourced claims in general — most of the web is unsourced; that alone proves nothing.

Keep human details: specific/unusual facts, mixed feelings and unresolved tension, dated/era-bound references, variety in sentence length, genuine asides and self-corrections.

## Output

- **Pasted text.** Return the rewrite. Note remaining AI patterns only if useful.
- **File.** Rewrite prose only. Leave code blocks, data, frontmatter, and link targets untouched. Summarize what changed.
- **Embedded** (PR description, commit message, a step inside another task or harness). Return the complete rewritten text and nothing else: no preamble, no summary of changes. Preserve the input's structure; headings, code blocks, data, and link targets stay verbatim as in file mode. If nothing needs changing, return the input unchanged.
