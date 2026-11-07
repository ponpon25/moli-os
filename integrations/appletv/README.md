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
  - `volume` (0-100) : seulement quand la télé ou l'ampli laisse l'Apple TV le régler.
- Appairage depuis le tableau de bord, une fois : `pairing` sur oui, l'Apple TV affiche un code à
  4 chiffres, l'écrire dans `pin`. L'appairage est gardé dans le coffre (`pairing`). Si l'Apple TV
  l'oublie (réinitialisation), le pilote repasse seul en attente d'appairage.
- Session vérifiée toutes les 30 s ; reconnexion avec attente croissante (2 s à 1 min), le port
  étant redemandé à chaque fois.
- Pas encore : ce qui joue (titre, image) et les sorties audio AirPlay, qui passent par un autre
  protocole (AirPlay/MRP).
