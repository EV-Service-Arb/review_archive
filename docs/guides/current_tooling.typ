#show link: it => underline(text(fill: blue, it))

= stuff already nice to use

== MCP plugin
Eric bot, - answers based on smart ranking of (skool, all chats, live calls, YT vids, etc).
Knows to respect authority of information source (10 levels) and adjust for age of the info.

#figure(
  image(".assets/playbook_live_call_advice.png"),
  caption: [prompt: "what I need for live call"],
)
#figure(
  image(".assets/playbook_eric_drive_link.png"),
  caption: [actioning it: following a lead from the answer],
)

*setup*: it follows the global standard, so register acc on #link("https://sav.evinvest.ltd"), then:
- *claude*: run
  `claude mcp add --scope user --transport http service-arb https://sa.evinvest.ltd/playbook_mcp`, or just tell your agent to do it
- *codex*:
  `codex mcp add service-arb --url https://sa.evinvest.ltd/playbook_mcp`, or again, - just tell it to do it
- *ChatGPT* in browser:
  Settings → Apps & Connectors → Advanced: turn on *Developer mode*. Then *Create*: name `service-arb`, URL #link("https://sa.evinvest.ltd/playbook_mcp")[`https://sa.evinvest.ltd/playbook_mcp`], authentication OAuth. Select it in a chat under *+* → Developer mode | *Connect* on the connector
- *anywhere* else:
  it's done by the global standard, just ask the agent

== review archive
#grid(columns: (1fr, 1.2fr), gutter: 1em)[
  before evil google nukes them: #link("https://sa.evinvest.ltd/review_archive")
  - make acc
  - dm me acc's email for test tokens // will add default starting amount bit later
  - add accs managing profiles under `managing gmails`
  - profit

  there is also ability to set up alerts and automatic email communications (alerts on eg google asking if our business still exists; automates spamming them to let us use Live Verif / reinstate profiles). But it's a bit raw
][#image(".assets/review_archive_overview_snap.png")]

== other
will finish with my profile verifs and I'll push other stuff I have to here. And we'll get the AI agent directly onto the site + being able to use all the tooling on it.
