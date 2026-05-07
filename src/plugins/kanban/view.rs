//! Kanban editor + read-only view.

use dioxus::prelude::*;

use super::model::{KanbanBoard, KanbanCard, KanbanColumn};

#[component]
pub fn KanbanView(board: KanbanBoard) -> Element {
    rsx! {
        div {
            class: "operon-kanban-view",
            "data-testid": "kanban-view",
            style: "display: flex; gap: 0.75rem; padding: 0.75rem; overflow: auto; height: 100%;",
            for col in board.columns.iter() {
                div {
                    key: "{col.id}",
                    class: "operon-kanban-column",
                    style: "min-width: 14rem; padding: 0.5rem; background: var(--operon-panel, #1a1a1a); border: 1px solid var(--operon-border, #333); border-radius: 0.25rem; display: flex; flex-direction: column; gap: 0.5rem;",
                    h3 { style: "margin: 0; font-size: 0.95em;", "{col.title}" }
                    for card in col.cards.iter() {
                        div {
                            key: "{card.id}",
                            class: "operon-kanban-card",
                            style: "padding: 0.4rem 0.5rem; background: var(--operon-bg, #111); border: 1px solid var(--operon-border, #333); border-radius: 0.25rem; white-space: pre-wrap;",
                            "{card.text}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn KanbanEditor(initial: String, on_change: EventHandler<String>) -> Element {
    let mut board: Signal<KanbanBoard> = use_signal(|| KanbanBoard::parse(&initial));

    let push_change = move |b: &KanbanBoard| {
        on_change.call(b.to_json());
    };

    let add_column = move |_| {
        board.with_mut(|b| {
            b.columns.push(KanbanColumn {
                id: KanbanBoard::fresh_id(),
                title: "Untitled".into(),
                cards: Vec::new(),
            });
        });
        let snap = board.read().clone();
        push_change(&snap);
    };

    rsx! {
        div {
            class: "operon-kanban-editor",
            "data-testid": "kanban-editor",
            style: "display: flex; gap: 0.75rem; padding: 0.75rem; overflow: auto; height: 100%; align-items: flex-start;",
            for (col_idx, col) in board.read().columns.iter().enumerate() {
                {
                    let col_id = col.id.clone();
                    let col_title = col.title.clone();
                    let cards = col.cards.clone();
                    rsx! {
                        ColumnView {
                            key: "{col_id}",
                            col_idx: col_idx,
                            col_id: col_id.clone(),
                            title: col_title,
                            cards,
                            board: board,
                            on_persist: EventHandler::new(move |json: String| on_change.call(json)),
                        }
                    }
                }
            }
            button {
                r#type: "button",
                class: "operon-button",
                "data-testid": "kanban-add-column",
                style: "padding: 0.5rem 0.75rem; height: fit-content; align-self: flex-start; border-radius: 0.25rem; cursor: pointer;",
                onclick: add_column,
                "+ Add column"
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ColumnViewProps {
    col_idx: usize,
    col_id: String,
    title: String,
    cards: Vec<KanbanCard>,
    board: Signal<KanbanBoard>,
    on_persist: EventHandler<String>,
}

#[component]
fn ColumnView(props: ColumnViewProps) -> Element {
    let col_idx = props.col_idx;
    let mut board = props.board;
    let on_persist = props.on_persist;

    let rename_column = move |evt: Event<FormData>| {
        let new_title = evt.value();
        board.with_mut(|b| {
            if let Some(c) = b.columns.get_mut(col_idx) {
                c.title = new_title;
            }
        });
        let snap = board.read().clone();
        on_persist.call(snap.to_json());
    };

    let add_card = move |_| {
        board.with_mut(|b| {
            if let Some(c) = b.columns.get_mut(col_idx) {
                c.cards.push(KanbanCard {
                    id: KanbanBoard::fresh_id(),
                    text: String::new(),
                });
            }
        });
        let snap = board.read().clone();
        on_persist.call(snap.to_json());
    };

    let delete_column = move |_| {
        board.with_mut(|b| {
            if col_idx < b.columns.len() {
                b.columns.remove(col_idx);
            }
        });
        let snap = board.read().clone();
        on_persist.call(snap.to_json());
    };

    rsx! {
        div {
            class: "operon-kanban-column",
            "data-testid": "kanban-column",
            "data-col-id": "{props.col_id}",
            style: "min-width: 16rem; max-width: 16rem; padding: 0.5rem; background: var(--operon-panel, #1a1a1a); border: 1px solid var(--operon-border, #333); border-radius: 0.25rem; display: flex; flex-direction: column; gap: 0.4rem;",
            div {
                style: "display: flex; align-items: center; gap: 0.4rem;",
                input {
                    r#type: "text",
                    "data-testid": "kanban-column-title",
                    value: "{props.title}",
                    style: "flex: 1; background: transparent; border: 0; color: inherit; font-weight: 600; font-size: 0.95em; padding: 0.15rem;",
                    onchange: rename_column,
                }
                button {
                    r#type: "button",
                    "data-testid": "kanban-column-delete",
                    "aria-label": "Delete column",
                    style: "background: transparent; border: 0; color: inherit; opacity: 0.5; cursor: pointer; padding: 0.15rem 0.35rem;",
                    onclick: delete_column,
                    "×"
                }
            }
            for (card_idx, card) in props.cards.iter().enumerate() {
                {
                    let card_id = card.id.clone();
                    let text = card.text.clone();
                    rsx! {
                        CardView {
                            key: "{card_id}",
                            col_idx: col_idx,
                            card_idx: card_idx,
                            card_id: card_id.clone(),
                            text,
                            board: board,
                            on_persist: on_persist,
                        }
                    }
                }
            }
            button {
                r#type: "button",
                class: "operon-button",
                "data-testid": "kanban-add-card",
                style: "padding: 0.35rem; border-radius: 0.25rem; cursor: pointer; opacity: 0.7;",
                onclick: add_card,
                "+ Add card"
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct CardViewProps {
    col_idx: usize,
    card_idx: usize,
    card_id: String,
    text: String,
    board: Signal<KanbanBoard>,
    on_persist: EventHandler<String>,
}

#[component]
fn CardView(props: CardViewProps) -> Element {
    let col_idx = props.col_idx;
    let card_idx = props.card_idx;
    let mut board = props.board;
    let on_persist = props.on_persist;

    let edit_card = move |evt: Event<FormData>| {
        let v = evt.value();
        board.with_mut(|b| {
            if let Some(col) = b.columns.get_mut(col_idx) {
                if let Some(card) = col.cards.get_mut(card_idx) {
                    card.text = v;
                }
            }
        });
        let snap = board.read().clone();
        on_persist.call(snap.to_json());
    };

    let delete_card = move |_| {
        board.with_mut(|b| {
            if let Some(col) = b.columns.get_mut(col_idx) {
                if card_idx < col.cards.len() {
                    col.cards.remove(card_idx);
                }
            }
        });
        let snap = board.read().clone();
        on_persist.call(snap.to_json());
    };

    rsx! {
        div {
            class: "operon-kanban-card",
            "data-testid": "kanban-card",
            "data-card-id": "{props.card_id}",
            style: "padding: 0.35rem 0.4rem; background: var(--operon-bg, #111); border: 1px solid var(--operon-border, #333); border-radius: 0.25rem; display: flex; gap: 0.25rem; align-items: flex-start;",
            textarea {
                "data-testid": "kanban-card-text",
                rows: "2",
                value: "{props.text}",
                style: "flex: 1; resize: vertical; background: transparent; border: 0; color: inherit; font-family: inherit; font-size: 0.9em;",
                onchange: edit_card,
            }
            button {
                r#type: "button",
                "data-testid": "kanban-card-delete",
                "aria-label": "Delete card",
                style: "background: transparent; border: 0; color: inherit; opacity: 0.4; cursor: pointer; padding: 0;",
                onclick: delete_card,
                "×"
            }
        }
    }
}
