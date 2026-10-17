# Pattern catalogue

Every pattern the `humanizer` skill checks, with words to watch and a before and after. Numbers are stable: other notes cite them. Judge density, not single words.

## CONTENT PATTERNS

### 1. Undue Emphasis on Significance, Legacy, and Broader Trends

**Words to watch:** stands/serves as, is a testament/reminder, a vital/significant/crucial/pivotal/key role/moment, underscores/highlights its importance/significance, reflects broader, symbolizing its ongoing/enduring/lasting, contributing to the, setting the stage for, marking/shaping the, represents/marks a shift, key turning point, evolving landscape, focal point, indelible mark, deeply rooted

**Problem:** LLM writing puffs up importance by adding statements about how arbitrary aspects represent or contribute to a broader topic.

**Before:**

> The Statistical Institute of Catalonia was officially established in 1989, marking a pivotal moment in the evolution of regional statistics in Spain. This initiative was part of a broader movement across Spain to decentralize administrative functions and enhance regional governance.

**After:**

> The Statistical Institute of Catalonia was established in 1989 to collect and publish regional statistics independently from Spain's national statistics office.

---

### 2. Undue Emphasis on Notability and Media Coverage

**Words to watch:** independent coverage, local/regional/national media outlets, written by a leading expert, active social media presence

**Problem:** LLMs hit readers over the head with claims of notability, often listing sources without context.

**Before:**

> Her views have been cited in The New York Times, BBC, Financial Times, and The Hindu. She maintains an active social media presence with over 500,000 followers.

**After:**

> In a 2024 New York Times interview, she argued that AI regulation should focus on outcomes rather than methods.

---

### 3. Superficial Analyses with -ing Endings

**Words to watch:** highlighting/underscoring/emphasizing..., ensuring..., reflecting/symbolizing..., contributing to..., cultivating/fostering..., encompassing..., showcasing...

**Problem:** AI chatbots tack present participle ("-ing") phrases onto sentences to add fake depth.

**Before:**

> The temple's color palette of blue, green, and gold resonates with the region's natural beauty, symbolizing Texas bluebonnets, the Gulf of Mexico, and the diverse Texan landscapes, reflecting the community's deep connection to the land.

**After:**

> The temple uses blue, green, and gold colors. The architect said these were chosen to reference local bluebonnets and the Gulf coast.

---

### 4. Promotional and Advertisement-like Language

**Words to watch:** boasts a, vibrant, rich (figurative), profound, enhancing its, showcasing, exemplifies, commitment to, natural beauty, nestled, in the heart of, groundbreaking (figurative), renowned, breathtaking, must-visit, stunning

**Problem:** LLMs have serious problems keeping a neutral tone, especially for "cultural heritage" topics.

**Before:**

> Nestled within the breathtaking region of Gonder in Ethiopia, Alamata Raya Kobo stands as a vibrant town with a rich cultural heritage and stunning natural beauty.

**After:**

> Alamata Raya Kobo is a town in the Gonder region of Ethiopia, known for its weekly market and 18th-century church.

---

### 5. Vague Attributions and Weasel Words

**Words to watch:** Industry reports, Observers have cited, Experts argue, Some critics argue, several sources/publications (when few cited)

**Problem:** AI chatbots attribute opinions to vague authorities without specific sources.

**Before:**

> Due to its unique characteristics, the Haolai River is of interest to researchers and conservationists. Experts believe it plays a crucial role in the regional ecosystem.

**After:**

> The Haolai River supports several endemic fish species, according to a 2019 survey by the Chinese Academy of Sciences.

---

### 6. Outline-like "Challenges and Future Prospects" Sections

**Words to watch:** Despite its... faces several challenges..., Despite these challenges, Challenges and Legacy, Future Outlook

**Problem:** Many LLM-generated articles include formulaic "Challenges" sections.

**Before:**

> Despite its industrial prosperity, Korattur faces challenges typical of urban areas, including traffic congestion and water scarcity. Despite these challenges, with its strategic location and ongoing initiatives, Korattur continues to thrive as an integral part of Chennai's growth.

**After:**

> Traffic congestion increased after 2015 when three new IT parks opened. The municipal corporation began a stormwater drainage project in 2022 to address recurring floods.

---

### 6a. Institutional Abstractions and Buzzword Noun Clusters

**Words to watch:** stakeholder engagement, transformative opportunities, innovative solutions, inclusive excellence, strategic initiatives, future workforce, critical thinking skills, functional design considerations, interpersonal care, team synergy

**Problem:** AI often stacks abstractions that sound official but never point to a person, object, scene, quote, or falsifiable claim.

**Before:**

> Effective collaboration requires not only interpersonal care but also the strategic navigation of team synergy.

**After:**

> The team worked better once each task had an owner and a deadline.

---

## LANGUAGE AND GRAMMAR PATTERNS

### 7. Overused "AI Vocabulary" Words

**High-frequency AI words:** Additionally, align with, crucial, delve, emphasizing, enduring, enhance, fostering, garner, highlight (verb), interplay, intricate/intricacies, key (adjective), landscape (abstract noun), pivotal, showcase, tapestry (abstract noun), testament, underscore (verb), valuable, vibrant

**Problem:** These words appear far more frequently in post-2023 text. They often co-occur.

**Before:**

> Additionally, a distinctive feature of Somali cuisine is the incorporation of camel meat. An enduring testament to Italian colonial influence is the widespread adoption of pasta in the local culinary landscape, showcasing how these dishes have integrated into the traditional diet.

**After:**

> Somali cuisine also includes camel meat, which is considered a delicacy. Pasta dishes, introduced during Italian colonization, remain common, especially in the south.

---

### 8. Avoidance of "is"/"are" (Copula Avoidance)

**Words to watch:** serves as/stands as/marks/represents [a], boasts/features/offers [a]

**Problem:** LLMs substitute elaborate constructions for simple copulas.

**Before:**

> Gallery 825 serves as LAAA's exhibition space for contemporary art. The gallery features four separate spaces and boasts over 3,000 square feet.

**After:**

> Gallery 825 is LAAA's exhibition space for contemporary art. The gallery has four rooms totaling 3,000 square feet.

---

### 9. Negative Parallelisms and Over-Balanced Contrasts

**Words to watch:** not only... but also..., it's not just..., rather than X..., instead of focusing on..., not merely... it's...

**Problem:** Constructions like "Not only...but..." or "It's not just about..., it's..." are overused. AI also loves tidy counterpoint sentences that sound balanced without saying anything concrete.

**Before:**

> It's not just about the beat riding under the vocals; it's part of the aggression and atmosphere. It's not merely a song, it's a statement.

**After:**

> The heavy beat adds to the aggressive tone.

**Before:**

> Rather than focusing on obstacles, we should embrace transformative opportunities.

**After:**

> The two blockers are budget and hiring.

---

### 10. Rule of Three Overuse

**Problem:** LLMs force ideas into groups of three to appear comprehensive.

**Before:**

> The event features keynote sessions, panel discussions, and networking opportunities. Attendees can expect innovation, inspiration, and industry insights.

**After:**

> The event includes talks and panels. There's also time for informal networking between sessions.

---

### 11. Elegant Variation (Synonym Cycling)

**Problem:** AI has repetition-penalty code causing excessive synonym substitution.

**Before:**

> The protagonist faces many challenges. The main character must overcome obstacles. The central figure eventually triumphs. The hero returns home.

**After:**

> The protagonist faces many challenges but eventually triumphs and returns home.

---

### 12. False Ranges

**Problem:** LLMs use "from X to Y" constructions where X and Y aren't on a meaningful scale.

**Before:**

> Our journey through the universe has taken us from the singularity of the Big Bang to the grand cosmic web, from the birth and death of stars to the enigmatic dance of dark matter.

**After:**

> The book covers the Big Bang, star formation, and current theories about dark matter.

---

## STYLE PATTERNS

### 13. Em Dash Overuse

**Problem:** LLMs use em dashes (—) more than humans, mimicking "punchy" sales writing.

In text you write yourself (commit messages, docs, replies), use none. End the sentence or use a comma. When editing someone else's text, cut the overuse and keep their voice.

**Before:**

> The term is primarily promoted by Dutch institutions—not by the people themselves. You don't say "Netherlands, Europe" as an address—yet this mislabeling continues—even in official documents.

**After:**

> The term is primarily promoted by Dutch institutions, not by the people themselves. You don't say "Netherlands, Europe" as an address, yet this mislabeling continues in official documents.

---

### 14. Overuse of Boldface

**Problem:** AI chatbots emphasize phrases in boldface mechanically.

**Before:**

> It blends **OKRs (Objectives and Key Results)**, **KPIs (Key Performance Indicators)**, and visual strategy tools such as the **Business Model Canvas (BMC)** and **Balanced Scorecard (BSC)**.

**After:**

> It blends OKRs, KPIs, and visual strategy tools like the Business Model Canvas and Balanced Scorecard.

---

### 15. Inline-Header Vertical Lists

**Problem:** AI outputs lists where items start with bolded headers followed by colons.

**Before:**

> - **User Experience:** The user experience has been significantly improved with a new interface.
> - **Performance:** Performance has been enhanced through optimized algorithms.
> - **Security:** Security has been strengthened with end-to-end encryption.

**After:**

> The update improves the interface, speeds up load times through optimized algorithms, and adds end-to-end encryption.

---

### 16. Title Case in Headings

**Problem:** AI chatbots capitalize all main words in headings.

**Before:**

> ## Strategic Negotiations And Global Partnerships

**After:**

> ## Strategic negotiations and global partnerships

---

### 17. Emojis

**Problem:** AI chatbots often decorate headings or bullet points with emojis.

**Before:**

> 🚀 **Launch Phase:** The product launches in Q3
> 💡 **Key Insight:** Users prefer simplicity
> ✅ **Next Steps:** Schedule follow-up meeting

**After:**

> The product launches in Q3. User research showed a preference for simplicity. Next step: schedule a follow-up meeting.

---

### 18. Curly Quotation Marks

**Problem:** ChatGPT uses curly quotes (“...”) instead of straight quotes ("...").

**Before:**

> He said “the project is on track” but others disagreed.

**After:**

> He said "the project is on track" but others disagreed.

---

## COMMUNICATION PATTERNS

### 19. Collaborative Communication Artifacts

**Words to watch:** I hope this helps, Of course!, Certainly!, You're absolutely right!, Would you like..., let me know, here is a...

**Problem:** Text meant as chatbot correspondence gets pasted as content.

**Before:**

> Here is an overview of the French Revolution. I hope this helps! Let me know if you'd like me to expand on any section.

**After:**

> The French Revolution began in 1789 when financial crisis and food shortages led to widespread unrest.

---

### 20. Knowledge-Cutoff Disclaimers

**Words to watch:** as of [date], Up to my last training update, While specific details are limited/scarce..., based on available information...

**Problem:** AI disclaimers about incomplete information get left in text.

**Before:**

> While specific details about the company's founding are not extensively documented in readily available sources, it appears to have been established sometime in the 1990s.

**After:**

> The company was founded in 1994, according to its registration documents.

---

### 21. Sycophantic/Servile Tone

**Problem:** Overly positive, people-pleasing language.

**Before:**

> Great question! You're absolutely right that this is a complex topic. That's an excellent point about the economic factors.

**After:**

> The economic factors you mentioned are relevant here.

---

### 21a. Institutional Voice That Could Be Anyone's

**Problem:** AI often sounds like a university mission statement, annual report, or committee memo. The tone is polished, comprehensive, and emotionally safe, but nobody in particular seems to be speaking.

**Diagnostic question:** Does this sound like a specific person wrote it, or like an institution trying to sound reasonable?

**Before:**

> By fostering awareness, encouraging dialogue among stakeholders, and implementing thoughtful strategies for responsible integration, we can ensure that technological innovation serves to enhance authentic communication.

**After:**

> If AI is going to be used here, someone still has to read the draft line by line and cut the dead language.

---

## FILLER AND HEDGING

### 22. Filler Phrases

**Before → After:**

- "In order to achieve this goal" → "To achieve this"
- "Due to the fact that it was raining" → "Because it was raining"
- "At this point in time" → "Now"
- "In the event that you need help" → "If you need help"
- "The system has the ability to process" → "The system can process"
- "It is important to note that the data shows" → "The data shows"

---

### 23. Excessive Hedging

**Problem:** Over-qualifying statements.

**Before:**

> It could potentially possibly be argued that the policy might have some effect on outcomes.

**After:**

> The policy may affect outcomes.

---

### 24. Generic Positive Conclusions

**Problem:** Vague upbeat endings.

**Before:**

> The future looks bright for the company. Exciting times lie ahead as they continue their journey toward excellence. This represents a major step in the right direction.

**After:**

> The company plans to open two more locations next year.

---

## ADDITIONAL TROPES FROM TROPES.FYI

Use these as an extra scan for patterns not already covered above. This section intentionally avoids repeating earlier rules on copula avoidance, generic conclusions, boldface, or em-dash cleanup. A single instance is not automatically bad. The giveaway is density: repeated use, multiple tropes in one paragraph, or an entire piece built from the same move.

### Word choice quick-scan

- **Magic adverbs:** quietly, deeply, fundamentally, remarkably, arguably. These often puff up a sentence without adding evidence.
- **Additional AI-vocabulary words:** certainly, leverage (as a verb), robust, streamline, harness. Prefer plain words when they say the same thing.
- **Additional ornate abstract nouns:** paradigm, synergy, ecosystem, framework when a simpler noun would do.

### Sentence-structure quick-scan

- **Countdown-style negative parallelism:** "Not X. Not Y. Just Z." and overused surprise-reveal lines like "not because X, but because Y."
- **Self-answered rhetorical questions:** "The result? A mess." or "The scary part? It scales."
- **Anaphora abuse:** the same sentence opening repeated three or more times in short succession.
- **Filler transitions:** "It's worth noting", "Importantly", "Interestingly", "Notably", "It bears mentioning".
- **Gerund fragment litany:** "Fixing bugs. Writing features. Shipping faster."
- **Short punchy fragments:** one-sentence paragraphs or fragments used over and over for manufactured emphasis.
- **Listicle in a trench coat:** "The first... The second... The third..." disguised as prose.

### Tone quick-scan

- **False suspense:** "Here's the kicker", "Here's the thing", "Here's what most people miss".
- **Patronizing analogy mode:** "Think of it as..." or "It's like a..." when the analogy makes the prose more generic.
- **Futurist invitation:** "Imagine a world where..."
- **False vulnerability:** polished self-awareness that sounds risk-free or performative.
- **Assertion in place of proof:** "The truth is simple", "History is clear", "The reality is obvious".
- **Grandiose stakes inflation:** every point becomes epochal, foundational, or world-changing.
- **Teacher mode for expert audiences:** "Let's break this down", "Let's unpack this", "Let's dive in".
- **Invented concept labels:** supervision paradox, acceleration trap, workload creep. Name the actual problem instead of coining a term.

### Formatting and composition quick-scan

- **Unicode decoration beyond quote cleanup:** fancy arrows, ornamental punctuation, or other hard-to-type glyphs unless the source style clearly calls for them.
- **Fractal summaries:** each section announces what it will do, does it, then summarizes itself again.
- **Dead metaphor repetition:** the same metaphor keeps resurfacing across the whole piece.
- **Historical analogy stacking:** a rapid-fire parade of companies, products, or revolutions to borrow authority.
- **One-point dilution:** the same thesis restated in several different frames without new information.
- **Content duplication:** repeated paragraphs or near-duplicate sections.

### Practical rule

If you spot three or more of these tells in one paragraph, rewrite that paragraph from scratch instead of line-editing it. Aim for writing that feels varied, specific, slightly imperfect, and clearly written by someone in particular.

---

## TECHNICAL PROSE

Docs, commit messages, code review, and design notes have tells of their own. These come from pstack's writing rules (Lauren Tan, MIT). Apply them on top of the patterns above whenever the text is technical.

### 25. Abstract Metaphor Nouns

**Words to watch:** substrate (base), wedge or wedge in (add), vector (way, method), locus, vantage, nexus, primitive as a noun, harness as a metaphor (a test harness is fine), surface as in "API surface", bedrock, scaffolding as a metaphor, modality, paradigm, gold-plating (more than the job needs), ratchet as a metaphor (the mechanism's real name, or "a limit that only tightens"), evacuate for moving code (move out), endgame (the last phase), north star, flywheel

**Problem:** These read as technical but usually have a plainer, concrete word. Pick the concrete word. When you find a new offender, add it to this list with its replacement.

### 26. Mid-Sentence Colons

**Problem:** Colons are fine before a list or an example. Used as a connector in the middle of a sentence, they add nothing.

**Before:**

> If you're coming from traditional automation: instead of registering event handlers, you describe conditions.

**After:**

> You describe when the scheduler should fire in plain English.

### 27. Feelings Stated as Mechanisms

**Problem:** "The database stays close at hand", "SQL you can read" and "types that follow your schema" name a feeling. Name the mechanism or a number instead: "printing a query shows the exact string sent to the database", "a column rename fails the build". Ask what the sentence tells the reader to do or know, then write that. If you cannot restate it as an instruction, a fact, or a number, cut it. If the sentence could appear unchanged in another project's docs, it says nothing about this one.

### 28. Dense Sentences

**Problem:** If the reader has to backtrack to parse a sentence, break it in two or drop clauses. One idea per sentence.

### 29. Passive Voice That Hides the Actor

**Problem:** "Queries are validated" hides who validates. Write "the compiler validates queries". Passive is fine only when the actor is unknown or does not matter.

### 30. Adverbs Propping Up Weak Verbs

**Problem:** "Runs quickly" becomes "is fast" or the number. "Significantly improves" becomes the measured delta. An adverb that props up a weak verb means the verb is wrong.

### 31. Mannered Prose

**Problem:** A metaphor or flourish where a literal phrase exists: aphorisms ("wire it or delete it"), personified code ("the plan holds it"), figurative verbs ("rides along", "stands on"), stock framing phrases. "A dial worth turning" becomes "a parameter worth varying".

### 32. Over-Compression

**Problem:** Dropped articles, verbless fragments, arrows, and abbreviations make the reader decode instead of read.

**Before:**

> Parser rejects bad date → exit 2, no write.

**After:**

> The parser rejects a bad date, exits with code 2, and writes nothing.
