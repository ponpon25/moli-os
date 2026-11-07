# Apple TV

Moli parle à une Apple TV comme la télécommande de l'iPhone (protocole Companion), sur le réseau
local, sans cloud.

- Un appareil par Apple TV. Points :
  - `power` (oui/non) : réveil ou veille. Réveillée, l'Apple TV allume la télé et passe sur son
    entrée par HDMI-CEC si « Contrôler les téléviseurs et les récepteurs » est activé dans
    Réglages › Télécommandes et appareils ;
  - `state` (lecture) : `asleep`, `screensaver`, `awake`, `idle`, poussé par l'Apple TV ;
  - `key` (écriture) : les touches de la télécommande (`up`, `down`, `left`, `right`, `select`,
    `menu`, `home`, `play_pause`, `volume_up`, `volume_down`, `channel_up`, `channel_down`,
    `guide`, `page_up`, `page_down`) ;
  - `app` (écriture) : ouvre une appli par son identifiant (`com.apple.TVWatchList`) ou un lien ;
    la liste des applis installées est dans le journal à chaque connexion (« apple tv apps ») ;
  - `media_state` (lecture) : `playing`, `paused`, `idle`, d'après ce que l'appli en cours
    accepte ;
  - `media_control` (écriture) : `play`, `pause`, `next`, `previous` ;
  - `volume` (0-100) : seulement quand la télé ou l'ampli laisse l'Apple TV le régler ;
  - `canal_channel` (écriture, si l'option `canal_channels` est donnée) : met une chaîne de CANAL+
    en direct, d'où que l'appli en soit (fermée, sur une autre page, une chaîne en cours). Réveil
    si besoin, ouverture de l'appli, lien de la grille « Toutes les chaînes » envoyé deux fois
    (l'appli qui démarre ignore le premier), Haut ×4 pour caler la sélection sur « Maintenant »,
    puis la vignette : 4 par ligne, la tuile Multi-Live d'abord. ~20 s en tout. `canal_channels`
    donne l'ordre de la grille de l'abonnement (après Multi-Live) ; une tuile d'actualité
    (« À la une », « Chaîne événement ») peut décaler celles qui la suivent. Aucun lien ne choisit
    la chaîne : l'Apple TV retire la partie `?…` des liens qu'elle transmet.
- Son (AirPlay) : une session « télécommande » AirPlay 2 (port 7000, même appairage, aucun
  nouveau code) donne les sorties audio de l'Apple TV et les change. Points `audio_output`
  (lecture), `tv_speakers` (ses propres haut-parleurs, la télé) et un interrupteur par enceinte
  de l'option `airplay = [{ name = "…", id = "AA:BB:CC:DD:EE:FF" }]` (`deviceid` du service
  `_airplay._tcp` de l'enceinte). Activer une enceinte retire « Haut-parleurs TV » (comme on le
  fait à la main : sinon la télé joue aussi et la télécommande ne règle plus le volume) ;
  couper la dernière rend la télé, l'Apple TV n'est jamais sans sortie.
- Appairage depuis le tableau de bord, une fois : `pairing` sur oui, l'Apple TV affiche un code à
  4 chiffres, l'écrire dans `pin`. L'appairage est gardé dans le coffre (`pairing`). Si l'Apple TV
  l'oublie (réinitialisation), le pilote repasse seul en attente d'appairage.
- Session vérifiée toutes les 30 s ; reconnexion avec attente croissante (2 s à 1 min), le port
  étant redemandé à chaque fois.
- Pas encore : ce qui joue (titre, image), que l'Apple TV pousse aussi (`NowPlayingInfo`).
