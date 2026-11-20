//! The Maps parser on saved pages. When Google changes the markup: refresh the fixtures
//! with `review_archive serve --dump-html <dir>` and a scan, fix `src/maps/selectors.rs`,
//! and `cargo insta review`.

use review_archive_core::maps::parse::{self, Card};

fn fixture(name: &str) -> Vec<Card> {
	let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
	parse::cards(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
}

/// What must hold on any live page, whatever the snapshot says.
fn invariants(cards: &[Card], expected: usize) {
	assert_eq!(cards.len(), expected, "one card per review, nested ids not double-counted");
	for c in cards {
		assert!(!c.id.is_empty(), "{c:?}");
		assert!(!c.author.is_empty(), "{c:?}");
		assert!(c.rating.is_some_and(|r| (1..=5).contains(&r)), "{c:?}");
		assert!(c.date_raw.is_some(), "{c:?}");
		assert!(c.author_url.as_deref().is_some_and(|u| u.contains("/maps/contrib/") && !u.contains('?')), "{c:?}");
	}
}

#[test]
fn fr_photos() {
	let cards = fixture("maps_fr_photos.html");
	invariants(&cards, 5);
	// three tiles and a "+ 5" tile
	assert_eq!(cards[0].photo_count, 8);
	assert!(cards[0].text.as_deref().unwrap().contains('\n'), "line breaks survive");
	insta::assert_yaml_snapshot!(cards, @r#"
	- id: Ci9DQUlRQUNvZENodHljRjlvT21vdFl5MWpSV05GVVZoaFVUTnZZM3BEV0dSSmVFRRAB
	  author: Cécile G
	  author_url: "https://www.google.com/maps/contrib/116351456539251048376/reviews"
	  rating: 5
	  date_raw: il y a 2 mois
	  text: "l'incontournable à Paris.\narrivée en soirée, pratiquement aucune attente, agréablement surpris.\nbeaucoup de monde par contre au second étage avec des touristes qui monopolisent la rambarde pour surfer sur internet et qui ne profitent pas de la vue.\nvous pouvez vous rendre au trocadero pour une belle vue de la tour ou au champ de Mars pour admirer l'illumination assis dans l’herbe"
	  reply: ~
	  photo_count: 8
	- id: Ci9DQUlRQUNvZENodHljRjlvT2s5UmVXdHdkMGQyZGtONVNISlRPRzQ0U0ZwcVNGRRAB
	  author: Nicolas Huguet
	  author_url: "https://www.google.com/maps/contrib/109437863301288287982/reviews"
	  rating: 5
	  date_raw: il y a un mois
	  text: "Visite absolument incontournable ! Monument exceptionnel et accessible. La vue au dernier étage est fantastique. Un conseil : regardez où vous déambuler car se cogner contre sa structure métallique est tout à fait possible en cheminant. J'en ai été quitte d'une belle bosse."
	  reply: ~
	  photo_count: 9
	- id: Ci9DQUlRQUNvZENodHljRjlvT25BeVRrVk9VMFpzWlZCamRTMXJNMEZLZVVKUlduYxAB
	  author: reg raguin
	  author_url: "https://www.google.com/maps/contrib/104808327632125814043/reviews"
	  rating: 5
	  date_raw: il y a 5 mois
	  text: "Super moment. J'y ai emmené mes petites filles de 8 et 4 ans et elles ont adoré. Nous avons pris l'ascenseur pour monter jusqu'au 2ième étage, nous avons profité des différentes vues de Paris puis nous sommes redescendues jusqu'au 1er afin de prendre un goûter. Le chocolat viennois est très bon et les bretzels sont pas non plus. Nous avons passé un très bon moment. Très bonne organisation."
	  reply: ~
	  photo_count: 21
	- id: Ci9DQUlRQUNvZENodHljRjlvT2psYVFYVm9RVW96VUY5R2NVSlBSSFZmZFdGS1MwRRAB
	  author: Sandrine Bole
	  author_url: "https://www.google.com/maps/contrib/112093436857752803182/reviews"
	  rating: 5
	  date_raw: il y a 2 mois
	  text: "C'est merveilleux, gigantesque et fantastique 🥰🤗 soit vous visitez en montant par l'escalier c'est gratuit... Nous avons payé car nous avons pris l'ascenseur, donc le premier étage 15 €le deuxième étage 24 €et le troisième 36 €... Honnêtement c'est vraiment avoir car on voit vraiment tout Paris🗼🎥💞"
	  reply: ~
	  photo_count: 5
	- id: Ci9DQUlRQUNvZENodHljRjlvT2xaalVYZ3hNazFET1hsaGJWVTFVME5VZGpBeE5HYxAB
	  author: Jc Battut
	  author_url: "https://www.google.com/maps/contrib/115023102613883312974/reviews"
	  rating: 5
	  date_raw: il y a 5 mois
	  text: "La Tour Eiffel est bien plus qu’un emblème de Paris : c’est une construction mythique qui fascine à chaque regard. Pour les amoureux de charpente métallique, c’est un véritable chef-d’œuvre ; on ne sait littéralement plus où poser les yeux tant chaque détail de structure impressionne. Entre élégance, prouesse technique et puissance visuelle, le lieu dégage une émotion unique. Une visite incontournable, aussi belle pour les passionnés d’architecture que pour les simples admirateurs de Paris."
	  reply: ~
	  photo_count: 35
	"#);
}

#[test]
fn fr_replies() {
	let cards = fixture("maps_fr_replies.html");
	invariants(&cards, 6);
	assert!(cards.iter().any(|c| c.reply.is_some()));
	for c in &cards {
		if let (Some(text), Some(reply)) = (&c.text, &c.reply) {
			assert!(!text.contains(reply.as_str()), "the reply is not part of the review text: {c:?}");
		}
	}
	insta::assert_yaml_snapshot!(cards, @r#"
	- id: Ci9DQUlRQUNvZENodHljRjlvT2tsNU1DMVhZbVpEZDJWSldtYzBaMlJOUTNKWFJHYxAB
	  author: Livia Purciuc
	  author_url: "https://www.google.com/maps/contrib/107230204969829222504/reviews"
	  rating: 4
	  date_raw: il y a 6 heures
	  text: ~
	  reply: "Bonjour Livia, merci beaucoup pour votre visite et pour ce retour qui nous fait très plaisir. N'hésitez pas à compléter votre avis en commentaire si vous le souhaitez. Nous espérons vous revoir très bientôt pour de nouvelles découvertes gustatives ! ✨ L'équipe Hippopotamus"
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2tVdGNsYzFPRVo0YlVWSmEzQXhTRTU2V0VkTVVVRRAB
	  author: Michael Campagne
	  author_url: "https://www.google.com/maps/contrib/111019439859047558140/reviews"
	  rating: 5
	  date_raw: il y a 19 heures
	  text: ~
	  reply: "Bonjour, merci pour cette belle reconnaissance de notre travail. N'hésitez pas à nous partager le détail de votre expérience en commentaire. Nous serions ravis de vous recevoir de nouveau très prochainement. L'équipe du restaurant Hippopotamus Paris Opéra 2e"
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2xKaFJ6UTNTM1JyYkVSaWFsRldjek0zVlRkaFVtYxAB
	  author: Lilith Braham
	  author_url: "https://www.google.com/maps/contrib/117144675590193679686/reviews"
	  rating: 3
	  date_raw: il y a 21 heures
	  text: Modeste est très gentille
	  reply: ~
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2s1VFJGRTJabWsyWTFGMU1tRXlhamgwZWpSNGRVRRAB
	  author: clara ni
	  author_url: "https://www.google.com/maps/contrib/112522216729078010314/reviews"
	  rating: 5
	  date_raw: il y a un jour
	  text: "Très bon service, le personnel très chaleureux."
	  reply: "Bonjour Clara ! Votre satisfaction est notre priorité, et votre message nous fait très plaisir. Merci ! Au plaisir de vous accueillir de nouveau pour un prochain repas. L'équipe Hippopotamus Paris Opéra 2e"
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT21waVIxUjBaMDlvVFZOWWRtRnhhMFpoYVRGWFRYYxAB
	  author: dominic rowland
	  author_url: "https://www.google.com/maps/contrib/101435523823098711596/reviews"
	  rating: 1
	  date_raw: il y a 2 jours
	  text: Nourriture et service déplorables. Certains membres de notre groupe ont été malades après le repas.
	  reply: ~
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2tseWEyTjNVMlp4WWsxYVFXbEtkWHBIT0V0bmMzYxAB
	  author: Paolo Maresu
	  author_url: "https://www.google.com/maps/contrib/112494106283481522695/reviews"
	  rating: 5
	  date_raw: il y a 2 jours
	  text: "Nous avons bien mangé et le service était rapide ; l’endroit est propre et bien tenu (y compris les toilettes). Nous nous sommes arrêtés sans réservation, à deux avec un jeune enfant, et ce fut une halte agréable. Il y a une table à langer dans les toilettes pour personnes handicapées au rez-de-chaussée."
	  reply: "Bonjour Paolo ! Un grand merci pour cette évaluation positive et encourageante. En espérant vous revoir très bientôt pour une nouvelle expérience gustative. L'équipe Hippopotamus Paris Opéra 2e"
	  photo_count: 0
	"#);
}

#[test]
fn en_replies() {
	let cards = fixture("maps_en_replies.html");
	invariants(&cards, 6);
	assert!(cards.iter().any(|c| c.reply.is_some()));
	insta::assert_yaml_snapshot!(cards, @r#"
	- id: Ci9DQUlRQUNvZENodHljRjlvT2tsNU1DMVhZbVpEZDJWSldtYzBaMlJOUTNKWFJHYxAB
	  author: Livia Purciuc
	  author_url: "https://www.google.com/maps/contrib/107230204969829222504/reviews"
	  rating: 4
	  date_raw: 6 hours ago
	  text: ~
	  reply: "Hello Livia, thank you so much for your visit and for this feedback, which we greatly appreciate. Feel free to add to your review in the comments if you wish. We hope to see you again very soon for more delicious discoveries! ✨ The Hippopotamus Team"
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2tVdGNsYzFPRVo0YlVWSmEzQXhTRTU2V0VkTVVVRRAB
	  author: Michael Campagne
	  author_url: "https://www.google.com/maps/contrib/111019439859047558140/reviews"
	  rating: 5
	  date_raw: 19 hours ago
	  text: ~
	  reply: "Hello, thank you for this wonderful recognition of our work. Please feel free to share the details of your experience in the comments. We would be delighted to welcome you back very soon. The Hippopotamus Paris Opéra 2nd Arrondissement Team"
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2xKaFJ6UTNTM1JyYkVSaWFsRldjek0zVlRkaFVtYxAB
	  author: Lilith Braham
	  author_url: "https://www.google.com/maps/contrib/117144675590193679686/reviews"
	  rating: 3
	  date_raw: 21 hours ago
	  text: Modeste is very kind
	  reply: ~
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2s1VFJGRTJabWsyWTFGMU1tRXlhamgwZWpSNGRVRRAB
	  author: clara ni
	  author_url: "https://www.google.com/maps/contrib/112522216729078010314/reviews"
	  rating: 5
	  date_raw: a day ago
	  text: "Excellent service, very friendly staff."
	  reply: "Hello Clara! Your satisfaction is our priority, and we are delighted by your message. Thank you! We look forward to welcoming you back for another meal. The Hippopotamus Paris Opéra 2nd team"
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT21waVIxUjBaMDlvVFZOWWRtRnhhMFpoYVRGWFRYYxAB
	  author: dominic rowland
	  author_url: "https://www.google.com/maps/contrib/101435523823098711596/reviews"
	  rating: 1
	  date_raw: 2 days ago
	  text: Horrible food and service. Some of our party were sick afterwards.
	  reply: ~
	  photo_count: 0
	- id: Ci9DQUlRQUNvZENodHljRjlvT2tseWEyTjNVMlp4WWsxYVFXbEtkWHBIT0V0bmMzYxAB
	  author: Paolo Maresu
	  author_url: "https://www.google.com/maps/contrib/112494106283481522695/reviews"
	  rating: 5
	  date_raw: 2 days ago
	  text: "We ate well and the service was prompt; the place is tidy and clean (including the bathroom). We stopped in without a reservation, two of us with a small child, and it was a pleasant stop. There's a changing table in the disabled restroom on the ground floor."
	  reply: Hello Paolo! Thank you so much for this positive and encouraging review. We hope to see you again very soon for another delicious experience. The Hippopotamus Paris Opéra 2nd team
	  photo_count: 0
	"#);
}

fn html(name: &str) -> String {
	let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
	std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn post(name: &str) -> Option<parse::Post> {
	parse::owner_post(&html(name))
}

#[test]
fn owner_posts() {
	let en = post("maps_en_owner_post.html").expect("the overview shows one");
	assert_eq!(en.date_raw.as_deref(), Some("6 hours ago"));
	assert!(en.text.starts_with("There's a plumbing emergency"), "{en:?}");
	assert!(en.text.ends_with("#SecureHome"), "{en:?}");
	assert!(en.text.contains("\n\n"), "paragraphs survive");

	let fr = post("maps_fr_owner_post.html").expect("the overview shows one");
	assert_eq!(fr.date_raw.as_deref(), Some("16 sept. 2026"));
	assert!(fr.text.starts_with("A Stay That Works Around You!"), "{fr:?}");

	assert_eq!(post("maps_en_overview_no_post.html"), None, "a reply to a review is not a post");
}

#[test]
fn place_names() {
	assert_eq!(parse::place_name(&html("maps_en_overview_no_post.html")).as_deref(), Some("ClearDrain Plumber Mississauga"));
	assert_eq!(
		parse::place_name(&html("maps_fr_limited_view.html")).as_deref(),
		Some("Tour Eiffel"),
		"not the sponsored heading beside it"
	);
	assert_eq!(
		parse::place_address(&html("maps_en_overview_no_post.html")).as_deref(),
		Some("1815 Lakeshore Rd W, Mississauga, ON L5J 1J6, Canada")
	);
	assert_eq!(parse::place_address(&html("maps_fr_limited_view.html")).as_deref(), Some("Av. Gustave Eiffel, 75007 Paris"));
}
