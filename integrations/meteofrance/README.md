# Vigilance Météo-France

- Un appareil « Vigilance » par département : la couleur du jour (`level`) et du lendemain
  (`level_tomorrow`), la couleur de chaque phénomène du jour (`wind`, `rain`, `storms`, `floods`,
  `snow_ice`, `heat`, `cold`, `avalanches`, `waves`) et une phrase (`summary`,
  `summary_tomorrow`) pour les messages. Valeurs : `green`, `yellow`, `orange`, `red`.
- Le littoral du département (`dd10` : vagues-submersion) compte avec lui.
- Lecture de `cartevigilance/encours` toutes les 10 minutes (`poll_secs`, 5 minutes au plus bas) ;
  la carte change deux fois par jour, et tout de suite quand une vigilance commence.
- Entre minuit et 6 h, la carte n'a pas de lendemain : `level_tomorrow` garde la valeur de la veille.
- Rien n'est envoyé par le pilote : une alerte est un automatisme sur `level` (canal messagerie).
- Reste : les textes des bulletins (`textesvigilance/encours`), l'outre-mer.
