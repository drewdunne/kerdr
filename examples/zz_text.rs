use kerdr::app::{App, Mode};
use kerdr::config::Config;
use kerdr::{demo, ui};
use ratatui::{Terminal, backend::TestBackend};
fn show(a: &App, w: u16, h: u16) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    let b = t.backend().buffer().clone();
    for y in 0..h { let mut l = String::new(); for x in 0..w { l.push_str(b[(x, y)].symbol()); } println!("{}", l.trim_end()); }
    println!("{}", "=".repeat(w as usize));
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (w, h): (u16, u16) = (args[1].parse().unwrap(), args[2].parse().unwrap());
    let mut a = App::new(Config::default(), demo::machines(false));
    a.demo = true; a.fixed_now = Some(demo::now()); a.width = w; a.rebuild();
    show(&a, w, h);
    let k = a.board.cards.iter().find(|c| c.reference == "#41").unwrap().key.clone();
    let i = a.card_index(&k).unwrap();
    a.sel.key = Some(k.clone()); a.sel.col = a.board.cards[i].stage.index();
    a.on_key(ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Char('o'), ratatui::crossterm::event::KeyModifiers::NONE));
    a.on_key(ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Char('j'), ratatui::crossterm::event::KeyModifiers::NONE));
    show(&a, w, h);
    if let Mode::Card{..} = a.mode {} else { println!("NOT CARD MODE {:?}", a.mode) }
}
