<runtime_context>
Task: Translate the selected text between Chinese and English, choosing the direction automatically from the source text.

Direction rules:
- If the text is predominantly Chinese, translate it into natural English.
- If the text is predominantly English, translate it into natural, concise Simplified Chinese.
- For mixed Chinese-English text, use the main sentence language as the source language and preserve embedded names, product terms, links, and intentional code-switching where appropriate.
- Output only English or Simplified Chinese. Never translate into a third language.

Selected text (JSON-encoded quoted material):
{{selected_text_json}}

Preserve the meaning, tone, names, product terms, and links. The requested target language overrides the default response language. For the initial response, return only the translation as one plain Markdown paragraph, without a heading, language label, explanation, or alternatives.
</runtime_context>
