# The welcome tour

Code: `crates/workspace/src/tour/` (`mod.rs`: the cards, which of them show, the dialog; `art.rs`: their pictures). Opened by the app at its start (`ph_workspace::tour::open_if_due`).

PilotHouse has much that someone who came for a chat with an agent does not go looking for: a phone beside the chat, tests recorded from what an agent did, decks of slides. A list of features in a document is not read. So the app says it once: a few cards, a picture that plays and three short lines each, read with **Next** to the last one's **Done**.

## What shows, and when

Each card has an id. The ids a user has gone through are kept (the setting `tour.seen`), and at a start the cards not yet seen show:

| | |
|---|---|
| After an install | Every card: *Welcome to PilotHouse* |
| After an update that added cards | Those alone: *What's new* |
| After an update that added none (a fix) | Nothing |

So a release says it has something new by adding a card, and by nothing else: no version is compared. Someone who installs later meets the new card in its place among the others, not at the end as news.

A card about a feature the build has switched off (`[features]` in `brand/brand.toml`) is left out, and so is a single line of a card that names another feature than the card's (the phone on the preview card needs `devices`, Chrome there `chrome_extension`, Utilities `utilities`, slides `presentations`, quick chat `quick_chat`, schedules `schedules`: `Card::needs`). The whole tour is the `tour` key.

## No way round it

At a start the dialog has no close button, a click beside it does nothing, and neither does Esc: the way out is the last card's *Done*. → , Enter and Space go on and ← goes back, but the keys only go through the cards: on the last one they do nothing, so a key held or pressed once too often does not close a card unread. Only *Done* notes the cards as seen, so a tour left by quitting the app shows again.

That is a lot to ask of someone who wanted to start working, and it is why the cards are few and short. A test holds them to it: eight cards at most, a title of at most 44 characters, three points of at most 72.

*Help → Welcome tour* (the ⋯ menu, and the palette) shows every card again, and so does **Show** beside *The welcome tour* in *Settings → About* (Settings closes to make way; searching Settings for *tour* finds it); asked for, the dialog closes like any other (its close button, Esc, a click beside it). Enter is still the next card there, not the dialog's answer.

## The cards

In the order they are read: what the app is, where things are, then what it has that a terminal with an agent does not (the most surprising first), and the small things last. Each says where in the app it is found, in the words the app uses there.

| Card | Says | Needs |
|---|---|---|
| `agents` | The five agents, each ticked when it is on this computer, one for each chat; a session's two forms, the chat and the agent's own CLI (**Chat \| CLI**), which are one conversation; no sign-up, nothing tracked | |
| `places` | Workspaces, Chats and the Bridge, and how they differ | |
| `preview` | The built-in browser, the user's own Chrome with its sign-ins (the extension) and an Android phone, all used by the agent | `browser` |
| `tests` | The project's tests in one list with *Fix with agent*, UI tests the agent records, and Utilities | `tests` |
| `review` | GitHub pull requests waiting for the user, read by an agent, the findings ticked and posted as one review | `github` |
| `artifacts` | Slides (to PowerPoint or PDF) and small apps, asked for in a chat, shared on the network | `artifacts` |
| `studio` | A picture edited by saying what to do, or by hand; a video compressed to fit a size (*At most 10 MB*), cut, sped up and exported | |
| `everyday` | The search, quick chat, schedules, themes | |

**Writing one.** A title that says what one can do, not the name of a place; the place goes small above it. Three lines at most, each one thing, in plain words. Say only what the app does as it says it (the button is called *Fix with agent*, so the card says that). Then add it to `CARDS` where someone new should meet it, with an id that was never used.

**Reading it as someone new.** The words were tested on a reader who knew nothing of the app: a small model given the cards' text alone (no picture, no code) and asked what each card lets one do and which words it had to guess at. What it guessed wrong was rewritten: *one conversation* (of the chat and the CLI) became *carry on where you were*, *the column of icons* became *the icons on the left*, the Bridge got *all your tasks on one board*, *Artifacts* got *what agents made for you*, *steps* became *numbered steps*, and sharing says that nothing goes on the internet. Do the same for a new card: a name the app made up (Bridge, Artifacts, Studio) needs a few plain words beside it the first time it is read.

## The pictures

Each is the app in small, doing the card's one thing: something said in the chat stands in the agent's CLI after the switch, and what is typed there is in the chat on the way back; the agent's pointer goes to a button of the page and presses it, a failed test gets *Fix with agent* and passes, an agent's findings are ticked and the review is posted, a deck is written slide by slide, an email on a screenshot is blurred. A picture that moves shows what happens; one that stands still only shows that something is there.

They are drawn in the app from the theme's colours (`art.rs`), not kept as image or video files: so they are sharp at any interface size, sit in a light theme as in a dark one and in a theme of the user's own, and weigh nothing. Each has a ground of two of the theme's base colours, so the cards are told apart at a glance. GitHub's mark on the review card is the icon set's (`Icons::Github`).

**How one plays.** A picture is a function of where its loop stands (0 to 1); `playing` asks for it again at each frame, thirty times a second, for a loop of nine to eleven seconds. What happens is written as moments of the loop (`span(at, 0.12, 0.36)`: how far along something is that happens between those two). A card that comes to the front plays from the start, so what happens is seen in its order; the loop ends on the finished picture and rests there for a moment before it goes round. With the system's *reduce motion* nothing plays and the finished picture shows, still (`REST`). Only the card in front plays.

They are laid out for a box of 612 by 300 pixels at the default interface size, in rems.

## Working on it

`PILOTHOUSE_NO_TOUR=1` starts the app without the cards (a scratch profile would show them at every start, and they are in the way of a test that drives the window). The buttons are `tour-next`, `tour-back` and `tour-done`; the action is `ShowTour`. To see the cards again at a start, take `tour.seen` out of the settings.
