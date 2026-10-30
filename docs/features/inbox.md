# Inbox and to-dos

Code: `crates/workspace/src/inbox.rs`, `crates/store/src/todos.rs`, agent questions from `ph_agent::approvals`. The bell in the title bar; its badge counts what needs you.

**Inbox** gathers, across workspaces:

- **Waiting for you**: agents' permission questions, answered right there.
- **Reminders**: to-dos whose reminder came due; **Done**, or **Remind me in 1 h**.
- **Finished**: schedules that ran since you last pressed **Clear**, with their result and **Open chat**.

**To-dos** is your own short list for the whole app: type and press Enter (or **Add**). **Remind me** picks when to be told (in 10 min, in 1 hour, today 18:00 — tomorrow's if it has passed —, tomorrow 09:00), or **Pick a time…** for one typed: `14:30`, `tomorrow 9am`, `fri 10:00`, `in 20 min`. A to-do an agent added for a workspace shows that workspace's name. Done ones stay a while, struck through.

A reminder is announced once when it comes due (on the schedules' 30-second tick), in the window and from the system, and stays in the inbox until done or moved.

Agents can leave you a to-do for something only you can do (sign in somewhere, decide something) with `todo_add`, optionally `remind_in_minutes`; `todo_list` shows them (control methods `todo.add`, `todo.list`).
