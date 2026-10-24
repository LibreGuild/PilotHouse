# Inbox and to-dos

Code: `crates/workspace/src/inbox.rs`, `crates/store/src/todos.rs`, agent questions from `ph_agent::approvals`. The bell in the title bar; its badge counts what needs you.

**Inbox** gathers, across workspaces:

- **Waiting for you**: agents' permission questions, answered right there.
- **Reminders**: to-dos whose reminder came due; **Done**, or **Remind me in 1 h**.
- **Finished**: schedules that ran since you last pressed **Clear**, with their result and **Open chat**.

**To-dos** is your own short list: type and press Enter. Pick a reminder (in 1 h, 18:00 — tomorrow's if it has passed —, tomorrow 09:00), and **All workspaces** for one that is not about this workspace. The list shows this workspace's to-dos and the shared ones; done ones stay a while, struck through.

A reminder is announced once when it comes due (on the schedules' 30-second tick), in the window and from the system, and stays in the inbox until done or moved.

Agents can leave you a to-do for something only you can do (sign in somewhere, decide something) with `todo_add`, optionally `remind_in_minutes`; `todo_list` shows them (control methods `todo.add`, `todo.list`).
