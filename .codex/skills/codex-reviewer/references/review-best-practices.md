# Review Best Practices (Condensed)

This file summarizes practical review practices used to design `codex-reviewer`.

## 1. Optimize for code health, not personal preference
- Use the Google standard: approve when the change improves overall code health, even if not perfect.
- Focus on correctness, maintainability, and user impact.

Source:
- https://google.github.io/eng-practices/review/reviewer/standard.html

## 2. Prioritize high-impact findings
- Look first for bugs, security issues, and regressions.
- Keep comments objective and specific.

Source:
- https://google.github.io/eng-practices/review/reviewer/
- https://google.github.io/eng-practices/review/reviewer/comments.html

## 3. Keep review loops small and fast
- Encourage small, focused changes and timely feedback to reduce risk and context switching.

Source:
- https://google.github.io/eng-practices/review/reviewer/speed.html

## 4. Use a repeatable security checklist
- Review authentication/authorization, input validation, data protection, and error handling.

Source:
- https://cheatsheetseries.owasp.org/cheatsheets/Code_Review_Cheat_Sheet.html

## 5. Maintain rigor with structured feedback
- Modern code review research highlights defect finding, maintainability, and knowledge sharing as primary outcomes.

Source:
- https://dl.acm.org/doi/10.1145/2597073.2597115
