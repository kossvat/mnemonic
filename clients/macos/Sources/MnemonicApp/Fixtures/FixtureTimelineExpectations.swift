enum FixtureTimelineExpectations {
    private static let ownerZero = " owner-first=No owner activity recorded "
    private static let hourTotals = "totals=[You 0m | Agents 1h] projects=[harbor=You 0m, Agents 1h] live=not-served empty=none "
    private static let overlapRecords = "intervals=Recorded intervals (2) folded records=[segment-1@540.00..600.00 | segment-2@550.00..590.00]"
    private static let long = " Beacon Orchard Lantern Meadow Beacon Orchard Lantern Meadow Beacon Orchard Lantern Meadow Beacon Orchard Lantern Meadow"

    static let byID: [String: String] = [
        "timeline-registry":
            "scale=Focus day=1440.00m span=480.00..600.00m width=580.00 "
            + "lanes=[beacon:Beacon rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-1:C · Claude Code · 1h] | harbor:Harbor rows=1 agents= count=0 recorded segments marks=0 labels=[] message=No sessions that day]"
            + ownerZero + "totals=[You 0m | Agents 1h] projects=[atlas=You 0m, Agents 0m | beacon=You 0m, Agents 1h | harbor=You 0m, Agents 0m | juniper=You 0m, Agents 0m | ledger=You 0m, Agents 0m] "
            + "live=not-served empty=none intervals=Recorded intervals (1) folded records=[segment-1@540.00..600.00]",
        "timeline-focus":
            "scale=Focus day=1440.00m span=480.00..600.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=2 agents=Claude Code · Codex count=2 recorded segments marks=2 "
            + "labels=[segment-1:C · Claude Code · 1h | segment-2:X · Codex · 40m]]"
            + ownerZero + hourTotals + overlapRecords,
        "timeline-day":
            "scale=Full day day=1440.00m span=0.00..1440.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=2 agents=Claude Code · Codex count=2 recorded segments marks=2 labels=[segment-1:C | segment-2:X]]"
            + ownerZero + hourTotals + overlapRecords,
        "timeline-one":
            "scale=Focus day=1440.00m span=480.00..600.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-1:C · Claude Code · 1h]]"
            + ownerZero + hourTotals + "intervals=Recorded intervals (1) folded records=[segment-1@540.00..600.00]",
        "timeline-short":
            "scale=Full day day=1440.00m span=0.00..1440.00m width=1440.00 "
            + "lanes=[harbor:Harbor rows=1 agents=Claude Code · Codex count=5 recorded segments marks=5 labels=[]]"
            + ownerZero + "totals=[You 0m | Agents 4m] projects=[harbor=You 0m, Agents 4m] live=not-served empty=none "
            + "intervals=Recorded intervals (5) folded records=[segment-1@540.00..541.00 | segment-2@550.00..551.00 | segment-3@560.00..561.00 | segment-4@580.00..580.00 | segment-5@0.00..1.00]",
        "timeline-edge":
            "scale=Focus day=1440.00m span=540.00..660.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-1:C · Claude Code · 2h]]"
            + ownerZero + "totals=[You 0m | Agents 2h] projects=[harbor=You 0m, Agents 2h] live=not-served empty=none "
            + "intervals=Recorded intervals (1) folded records=[segment-1@480.00..600.00]",
        "timeline-loading": "loading=Loading timeline span=not-shown totals=not-shown",
        "timeline-off":
            "failure=Local API is off: Project data is unavailable while this profile's API is disabled. span=not-shown totals=not-shown",
        "timeline-unreachable":
            "failure=Daemon unavailable: The local daemon did not answer. Available data may be out of date. span=not-shown totals=not-shown",
        "timeline-unauthorised":
            "failure=Access unavailable: The daemon did not accept this profile's access token. span=not-shown totals=not-shown",
        "timeline-earlier":
            "failure=Daemon update needed: This daemon does not provide the requested project data. span=not-shown totals=not-shown",
        "timeline-undecodable":
            "failure=Project data unavailable: The daemon's answer could not be read. Available data may be out of date. span=not-shown totals=not-shown",
        "timeline-activity-only":
            "scale=Focus day=1440.00m span=500.00..620.00m width=580.00 lanes=[] owner-first=3 marks "
            + "totals=[You 1h | Agents 0m] projects=[beacon=You 10m, Agents 0m | harbor=You 30m, Agents 0m | unassigned=You 20m, Agents 0m] "
            + "live=not-served empty=No agent sessions recorded for this day. "
            + "intervals=Recorded intervals (3) folded records=[owner-1@540.00..560.00[harbor=1.00] | owner-2@560.00..580.00[harbor=0.50 | beacon=0.50] | owner-3@600.00..620.00[unassigned=1.00]]",
        "timeline-empty":
            "scale=Full day day=1440.00m span=0.00..1440.00m width=580.00 lanes=[]" + ownerZero
            + "totals=[You 0m | Agents 0m] projects=[harbor=You 0m, Agents 0m] live=not-served "
            + "empty=No activity recorded for this day. records=[]",
        "timeline-owner-unavailable":
            "scale=Focus day=1440.00m span=480.00..600.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=2 agents=Claude Code · Codex count=2 recorded segments marks=2 "
            + "labels=[segment-1:C · Claude Code · 1h | segment-2:X · Codex · 40m]] owner-first=Owner activity unavailable "
            + "totals=[You Unavailable | Agents 1h] projects=[harbor=You Unavailable, Agents 1h] live=not-served empty=none "
            + overlapRecords,
        "timeline-23h":
            "scale=Full day day=1380.00m span=0.00..1380.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-1:C]]"
            + ownerZero + hourTotals + "intervals=Recorded intervals (1) folded records=[segment-1@60.00..120.00]",
        "timeline-25h":
            "scale=Full day day=1500.00m span=0.00..1500.00m width=580.00 "
            + "lanes=[harbor:Harbor rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-1:C]]"
            + ownerZero + hourTotals + "intervals=Recorded intervals (1) folded records=[segment-1@60.00..120.00]",
        "timeline-many":
            "scale=Focus day=1440.00m span=550.00..670.00m width=580.00 lanes=[atlas:Atlas" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=1 labels=[segment-4:X · Codex · 1h] | beacon:Beacon" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=0 labels=[] message=No sessions in this focus | cobalt:Cobalt" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=1 labels=[segment-14:X · Codex · 1h] | harbor:Harbor" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=0 labels=[] message=No sessions in this focus | juniper:Juniper" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-5:C · Claude Code · 1h] | lantern:Lantern" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-7:C · Claude Code · 1h] | ledger:Ledger" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-3:C · 1h] | meadow:Meadow" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-9:C · Claude Code · 1h] | orchard:Orchard" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=1 labels=[segment-6:X · Codex · 1h] | pylon:Pylon" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=1 labels=[segment-10:X · Codex · 1h] | quarry:Quarry" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=1 labels=[segment-8:X · Codex · 1h] | tundra:Tundra" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-11:C · Claude Code · 1h] | vessel:Vessel" + long
            + " rows=1 agents=Codex count=1 recorded segment marks=1 labels=[segment-12:X · Codex · 1h] | willow:Willow" + long
            + " rows=1 agents=Claude Code count=1 recorded segment marks=1 labels=[segment-13:C · Claude Code · 1h]]"
            + ownerZero + "totals=[You 0m | Agents 3h 10m] "
            + "projects=[atlas=You 0m, Agents 1h | beacon=You 0m, Agents 1h | cobalt=You 0m, Agents 1h | harbor=You 0m, Agents 1h | juniper=You 0m, Agents 1h | lantern=You 0m, Agents 1h | ledger=You 0m, Agents 1h | meadow=You 0m, Agents 1h | orchard=You 0m, Agents 1h | pylon=You 0m, Agents 1h | quarry=You 0m, Agents 1h | tundra=You 0m, Agents 1h | vessel=You 0m, Agents 1h | willow=You 0m, Agents 1h] "
            + "live=not-served empty=none intervals=Recorded intervals (14) folded records=[segment-1@480.00..540.00 | segment-2@490.00..550.00 | segment-3@500.00..560.00 | segment-4@510.00..570.00 | segment-5@520.00..580.00 | segment-6@530.00..590.00 | segment-7@540.00..600.00 | segment-8@550.00..610.00 | segment-9@560.00..620.00 | segment-10@570.00..630.00 | segment-11@580.00..640.00 | segment-12@590.00..650.00 | segment-13@600.00..660.00 | segment-14@610.00..670.00]",
        "timeline-crowded":
            "scale=Focus day=1440.00m span=540.00..660.00m width=580.00 "
            + "lanes=[beacon:Beacon rows=1 agents=Claude Code count=1 recorded segment marks=0 labels=[] message=No sessions in this focus | harbor:Harbor rows=3 agents=Claude Code · Codex count=36 recorded segments marks=9 labels=[] folded=+27 more segments hidden=27 | ledger:Ledger rows=1 agents=Codex count=1 recorded segment marks=0 labels=[] message=No sessions in this focus]"
            + ownerZero + "totals=[You 0m | Agents 33m] projects=[beacon=You 0m, Agents 15m | harbor=You 0m, Agents 3m | ledger=You 0m, Agents 15m] live=not-served empty=none "
            + "intervals=Recorded intervals (38) folded records=[segment-1@540.00..541.00 | segment-2@550.00..551.00 | segment-3@560.00..561.00 | segment-4@540.00..541.00 | segment-5@550.00..551.00 | segment-6@560.00..561.00 | segment-7@540.00..541.00 | segment-8@550.00..551.00 | segment-9@560.00..561.00 | segment-10@540.00..541.00 | segment-11@550.00..551.00 | segment-12@560.00..561.00 | segment-13@540.00..541.00 | segment-14@550.00..551.00 | segment-15@560.00..561.00 | segment-16@540.00..541.00 | segment-17@550.00..551.00 | segment-18@560.00..561.00 | segment-19@540.00..541.00 | segment-20@550.00..551.00 | segment-21@560.00..561.00 | segment-22@540.00..541.00 | segment-23@550.00..551.00 | segment-24@560.00..561.00 | segment-25@540.00..541.00 | segment-26@550.00..551.00 | segment-27@560.00..561.00 | segment-28@540.00..541.00 | segment-29@550.00..551.00 | segment-30@560.00..561.00 | segment-31@540.00..541.00 | segment-32@550.00..551.00 | segment-33@560.00..561.00 | segment-34@540.00..541.00 | segment-35@550.00..551.00 | segment-36@560.00..561.00 | segment-37@660.00..675.00 | segment-38@780.00..795.00]"
    ]
}
