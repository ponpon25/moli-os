# Onboarding : Sonos

1. Toute la maison Sonos d'un coup : `discover = true` (une seule instance, un appareil par pièce, paires stéréo regroupées). Sinon : quel lecteur (pièce) et son adresse IP, une instance par lecteur (obligatoire pour les annonces).
2. Aucun identifiant pour la lecture et le volume.
3. Annonces (facultatif) : `[driver.options.announce]` avec `media_base` (Moli tel que
   l'enceinte le joint, `http://<ip de Moli>:8790`), `tts` (serveur Piper `hôte:port`),
   `voice`, `volume`. Il faut une clé maître (empreinte `ws_cert` épinglée au premier contact).
4. Déclarer `kind = "sonos"`, `discover = true` (ou `host = "<ip>"`) ; `moli-os check-config`, redémarrer.
5. Tester en journée, volume bas : pause puis lecture ; une annonce courte si configurée.
6. Apple Music (facultatif, avec `discover = true`) : une clé MusicKit (portail développeur Apple : Identifiers → Media IDs, puis Keys → Media Services). `[driver.options.apple]` avec `team_id` et `key_id` ; le fichier `.p8` dans le coffre par un tube (`secrets set <instance> apple_music_key`), jamais en clair. Il faut un favori Apple Music dans l'appli Sonos (Moli y lit le compte).
