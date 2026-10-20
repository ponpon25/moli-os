# Olvid

- Un appareil « Olvid » avec un point `notify` (texte, écriture) : Moli envoie le texte à chaque
  personne appairée, par le daemon officiel (gRPC), chiffré de bout en bout.
- Les automatismes l'utilisent par leur canal de message écrit (le canal historique nommé
  « telegram » écrit sur **tout** point `notify`) ; le récapitulatif du soir et les messages de la
  sonnette partent aussi vers Olvid.
- Appairage : points `pairing` (ouvre 15 min, code du foyer), `link`, `invitation`, `code` (celui
  du bot, à saisir dans Olvid), `sas` (celui d'Olvid, à saisir dans Moli), `contacts`, `forget`.
- Secrets dans le coffre : `admin_key` (donnée par la personne), `identity`, `client_key`, `people`
  (écrits par le pilote).
- Pas encore : la réception (parler à Moli depuis Olvid).
