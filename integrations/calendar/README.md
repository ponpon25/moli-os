# Calendrier scolaire

- Un appareil « Calendrier scolaire » : `school_day` (aujourd'hui est un jour d'école : du lundi au
  vendredi, ni férié ni en vacances), `school_day_tomorrow`, `vacation` et `vacation_name`,
  `public_holiday` et `public_holiday_name`, `next_vacation` (nom et premier jour).
- Le mercredi compte comme un jour d'école : quels jours un enfant y va vraiment, c'est
  l'automatisme qui le dit (jours du déclencheur).
- Vacances : jeu de données `fr-en-calendrier-scolaire` de data.education.gouv.fr, filtré sur
  l'académie (`location`). Les dates des enseignants sont écartées. Un « pont » d'un jour (fin
  égale au début) vaut ce jour-là.
- Jours fériés : `https://calendrier.api.gouv.fr/jours-feries/<zone>.json` (`metropole` par
  défaut, `alsace-moselle`, outre-mer).
- Lecture une fois par jour (nouvel essai 30 min après un échec), gardée dans
  `data/calendar-<académie>.json` : un matin sans Internet sait quand même quel jour on est.
  Au-delà de ce que les listes couvrent, l'appareil se dit injoignable plutôt que de deviner.
- Rien n'est commandé par le pilote.
